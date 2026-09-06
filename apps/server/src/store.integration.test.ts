import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import {
	randomBytes,
	createCipheriv,
	createDecipheriv,
	sign,
} from 'node:crypto';
import { SyncStore } from './store';
import { createApp } from './app';
import { digest, signingBytes } from './protocol';
import { fixture } from './protocol.test';

// Requires a dedicated test database. Every run uses unique IDs; never truncates user tables.
const url = process.env.NOURA_TEST_DATABASE_URL;
describe.skipIf(!url)('PostgreSQL encrypted operation service', () => {
	const suffix = crypto.randomUUID();
	const workspace = `w_${suffix}`;
	const f = fixture(`d_${suffix}`, workspace, `o_${suffix}`);
	const token = randomBytes(32).toString('base64url');
	const account = `a_${suffix}`;
	const store = new SyncStore(url!);
	const app = createApp(store, { origin: 'http://localhost:1900' });
	let actor: { deviceId: string; accountId: string; publicKey: string };
	beforeAll(async () => {
		await store.migrate();
		await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${`d_${suffix}`},${account},${f.publicKey})`;
		await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${`d_${suffix}`},now()+interval '1 hour')`;
		actor = await store.authenticate(token);
		await store.createWorkspace(actor, workspace);
		await store.createObject(actor, workspace, `o_${suffix}`);
	});
	afterAll(async () => {
		await store.close();
	});
	test('account request budget rejects excess traffic and resets in a new window', async () => {
		const limited = { ...actor, accountId: `rate_${crypto.randomUUID()}` };
		await store.db`INSERT INTO noura_rate_limits(account_id,window_start,count)
		 VALUES(${limited.accountId},floor(extract(epoch FROM now())/60)::bigint,120)`;
		await expect(store.rateLimit(limited)).rejects.toMatchObject({
			code: 'sync.rate_limited',
			status: 429,
		});
		await store.db`UPDATE noura_rate_limits SET window_start=window_start-1 WHERE account_id=${limited.accountId}`;
		await store.rateLimit(limited);
		const [row] =
			await store.db`SELECT count FROM noura_rate_limits WHERE account_id=${limited.accountId}`;
		expect(row!.count).toBe(1);
	});
	function request(path: string, body?: unknown, auth = token) {
		return app.request(path, {
			method: body === undefined ? 'GET' : 'POST',
			headers: {
				Authorization: `Bearer ${auth}`,
				'Content-Type': 'application/json',
			},
			...(body === undefined ? {} : { body: JSON.stringify(body) }),
		});
	}
	test('workspace retries preserve ownership and self key backup is signed, immutable and epoch checked', async () => {
		await store.createWorkspace(actor, workspace);
		const key = {
			workspaceId: workspace,
			objectId: `o_${suffix}`,
			epoch: 1,
			deviceId: actor.deviceId,
			signingDevice: actor.deviceId,
			wrappedKey: randomBytes(128).toString('base64'),
			signature: '',
		};
		function signed(value: typeof key) {
			return {
				...value,
				signature: sign(
					null,
					Buffer.from(
						JSON.stringify([
							'noura.sync.key',
							1,
							value.workspaceId,
							value.objectId,
							value.epoch,
							value.signingDevice,
							value.deviceId,
							value.wrappedKey,
						]),
					),
					f.keys.privateKey,
				).toString('base64'),
			};
		}
		const put = (body: unknown) =>
			app.request('/v1/keys/self', {
				method: 'PUT',
				headers: {
					Authorization: `Bearer ${token}`,
					'Content-Type': 'application/json',
				},
				body: JSON.stringify(body),
			});
		expect((await put(signed(key))).status).toBe(204);
		expect((await put(signed(key))).status).toBe(204);
		expect(
			(
				await put(
					signed({ ...key, wrappedKey: randomBytes(128).toString('base64') }),
				)
			).status,
		).toBe(409);
		expect((await put(signed({ ...key, epoch: 2 }))).status).toBe(409);
		expect(
			(
				await put({
					...signed(key),
					signature: randomBytes(64).toString('base64'),
				})
			).status,
		).toBe(400);
		expect((await put(signed({ ...key, deviceId: 'other' }))).status).toBe(403);
		const outsider = `outsider_${crypto.randomUUID()}`;
		await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${outsider},${outsider},${f.publicKey})`;
		await expect(
			store.createWorkspace(
				{ ...actor, deviceId: outsider, accountId: outsider },
				workspace,
			),
		).rejects.toMatchObject({ status: 403 });
	});
	test('committed changes wake bounded pulls and authorization is rechecked after waiting', async () => {
		const page = await store.pull(actor, workspace, '0');
		const read = request(
			`/v1/workspaces/${workspace}/operations?after=${page.cursor}&accessRevision=${page.accessRevision}&wait=25`,
		);
		// Observe the actual LISTEN registration; no timing assumption about connection startup.
		const ready = await store.watch(
			workspace,
			new AbortController().signal,
			2000,
		);
		try {
			await store.push(actor, workspace, [f.make()]);
			await ready.changed;
			const response = await read;
			expect(response.status).toBe(200);
			expect((await response.json()).operations.length).toBeGreaterThan(0);
		} finally {
			ready.close();
		}

		const device = `wait_${crypto.randomUUID()}`;
		const waitToken = randomBytes(32).toString('base64url');
		await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${device},${account},${f.publicKey})`;
		await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(waitToken)},${device},now()+interval '1 hour')`;
		const end = await store.pull(actor, workspace, '0');
		const pending = request(
			`/v1/workspaces/${workspace}/operations?after=${end.cursor}&accessRevision=${end.accessRevision}&wait=25`,
			undefined,
			waitToken,
		);
		await store.db`UPDATE noura_devices SET revoked=true WHERE id=${device}`;
		await store.db`SELECT pg_notify('noura_sync',${workspace})`;
		expect((await pending).status).toBe(401);
	});
	test('HTTP authentication, origin and payload validation fail closed', async () => {
		expect((await app.request('/ready')).status).toBe(200);
		expect(
			(
				await request(
					`/v1/workspaces/${workspace}/operations`,
					undefined,
					'bad',
				)
			).status,
		).toBe(401);
		expect(
			(
				await app.request('/health', {
					headers: { Origin: 'https://untrusted.example' },
				})
			).status,
		).toBe(403);
		expect(
			(
				await request(`/v1/workspaces/${workspace}/operations`, {
					operations: [{ ...f.make(), plaintext: 'secret' }],
				})
			).status,
		).toBe(400);
		expect(
			(
				await request(`/v1/workspaces/${workspace}/operations`, {
					operations: [],
				})
			).status,
		).toBe(400);
		await expect(
			store.push(actor, workspace, [f.make(undefined, 1, '1')]),
		).rejects.toThrow('sync.stale_policy');
	});
	test('an encrypted update survives server restart and decrypts only with the client key', async () => {
		const secret = `plaintext-sentinel-${crypto.randomUUID()}`;
		const key = randomBytes(32);
		const nonce = randomBytes(12);
		const cipher = createCipheriv('aes-256-gcm', key, nonce);
		const ciphertext = Buffer.concat([
			cipher.update(secret),
			cipher.final(),
			cipher.getAuthTag(),
		]);
		const op = {
			...f.make(),
			nonce: nonce.toString('base64'),
			ciphertext: ciphertext.toString('base64'),
		};
		op.signature = sign(null, signingBytes(op), f.keys.privateKey).toString(
			'base64',
		);
		const response = await request(`/v1/workspaces/${workspace}/operations`, {
			operations: [op],
		});
		expect(response.status).toBe(200);
		const reopened = new SyncStore(url!);
		try {
			const page = await reopened.pull(actor, workspace, '0');
			const fetched = page.operations.find(
				(x) => x.operationId === op.operationId,
			)!;
			const bytes = Buffer.from(fetched.ciphertext, 'base64');
			const decipher = createDecipheriv(
				'aes-256-gcm',
				key,
				Buffer.from(fetched.nonce, 'base64'),
			);
			decipher.setAuthTag(bytes.subarray(-16));
			expect(
				Buffer.concat([
					decipher.update(bytes.subarray(0, -16)),
					decipher.final(),
				]).toString(),
			).toBe(secret);
			const rows =
				await reopened.db`SELECT row_to_json(o) AS data FROM noura_operations o WHERE workspace_id=${workspace}`;
			expect(JSON.stringify(rows)).not.toContain(secret);
		} finally {
			await reopened.close();
		}
	});
	test('lost acknowledgments and simultaneous retries allocate exactly one sequence', async () => {
		const op = f.make();
		const results = await Promise.all(
			Array.from({ length: 8 }, () => store.push(actor, workspace, [op])),
		);
		expect(new Set(results.map((x) => x[0])).size).toBe(1);
		const [count] =
			await store.db`SELECT count(*) AS count FROM noura_operations WHERE workspace_id=${workspace} AND operation_id=${op.operationId}`;
		expect(count!.count).toBe('1');
		await expect(
			store.push(actor, workspace, [f.make(op.operationId)]),
		).rejects.toThrow('sync.operation_id_reused');
	});
	test('a bad member of a batch rolls back its earlier operation and quota accounting', async () => {
		const good = f.make();
		const stale = f.make(crypto.randomUUID(), 2);
		const [before] =
			await store.db`SELECT sequence,used_bytes FROM noura_workspaces WHERE id=${workspace}`;
		await expect(store.push(actor, workspace, [good, stale])).rejects.toThrow(
			'sync.stale_epoch',
		);
		const [after] =
			await store.db`SELECT sequence,used_bytes FROM noura_workspaces WHERE id=${workspace}`;
		expect(after).toEqual(before);
		const rows =
			await store.db`SELECT 1 FROM noura_operations WHERE workspace_id=${workspace} AND operation_id=${good.operationId}`;
		expect(rows.length).toBe(0);
	});
	test('quota rejection does not consume a sequence', async () => {
		await store.db`UPDATE noura_workspaces SET quota_bytes=used_bytes WHERE id=${workspace}`;
		try {
			await expect(store.push(actor, workspace, [f.make()])).rejects.toThrow(
				'sync.quota_exceeded',
			);
		} finally {
			await store.db`UPDATE noura_workspaces SET quota_bytes=1073741824 WHERE id=${workspace}`;
		}
	});
	test('partial recipients only receive their granted object', async () => {
		const recipient = { ...actor, accountId: `recipient_${suffix}` };
		// A distinct enrolled device prevents account/device impersonation in the fixture.
		const r = fixture(`r_${suffix}`, workspace, `o_${suffix}`);
		await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${`r_${suffix}`},${recipient.accountId},${r.publicKey})`;
		recipient.deviceId = `r_${suffix}`;
		recipient.publicKey = r.publicKey;
		await expect(store.pull(recipient, workspace, '0')).rejects.toThrow(
			'sync.forbidden',
		);
		await store.db`INSERT INTO noura_grants(workspace_id,object_id,account_id,role) VALUES(${workspace},${`o_${suffix}`},${recipient.accountId},'viewer')`;
		const hidden = fixture(actor.deviceId, workspace, `hidden_${suffix}`);
		await store.createObject(actor, workspace, `hidden_${suffix}`);
		const hiddenOp = hidden.make();
		hiddenOp.signature = sign(
			null,
			signingBytes(hiddenOp),
			f.keys.privateKey,
		).toString('base64');
		await store.push(actor, workspace, [hiddenOp]);
		const page = await store.pull(recipient, workspace, '0');
		expect(page.operations.length).toBeGreaterThan(0);
		expect(page.operations.every((x) => x.objectId === `o_${suffix}`)).toBe(
			true,
		);
		await expect(store.push(recipient, workspace, [r.make()])).rejects.toThrow(
			'sync.forbidden',
		);
		await store.db`DELETE FROM noura_grants WHERE workspace_id=${workspace} AND account_id=${recipient.accountId}`;
		await expect(store.pull(recipient, workspace, page.cursor)).rejects.toThrow(
			'sync.forbidden',
		);
	});
	test('revoked devices cannot authenticate or use an already-resolved actor', async () => {
		await store.db`UPDATE noura_devices SET revoked=true WHERE id=${actor.deviceId}`;
		try {
			await expect(store.authenticate(token)).rejects.toThrow(
				'sync.unauthorized',
			);
			await expect(store.push(actor, workspace, [f.make()])).rejects.toThrow(
				'sync.unauthorized',
			);
			await expect(store.pull(actor, workspace, '0')).rejects.toThrow(
				'sync.unauthorized',
			);
		} finally {
			await store.db`UPDATE noura_devices SET revoked=false WHERE id=${actor.deviceId}`;
		}
	});
});

describe.skipIf(!url)(
	'recipient key delivery without permission changes',
	() => {
		test('writers may supply immutable keys only to current authorized devices', async () => {
			const store = new SyncStore(url!);
			await store.migrate();
			const workspace = `keys_${crypto.randomUUID()}`;
			const object = 'shared_object';
			const fixtures = ['owner', 'editor', 'viewer', 'outsider'].map((role) => {
				const id = `${role}_${crypto.randomUUID()}`;
				const f = fixture(id, workspace, object);
				return {
					role,
					id,
					f,
					actor: { deviceId: id, accountId: id, publicKey: f.publicKey },
				};
			});
			const [owner, editor, viewer, outsider] = fixtures;
			try {
				for (const item of fixtures) {
					await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${item.id},${item.id},${item.f.publicKey})`;
				}
				await store.createWorkspace(owner!.actor, workspace);
				await store.createObject(owner!.actor, workspace, object);
				for (const item of [editor!, viewer!]) {
					await store.db`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${item.id},${item.role})`;
				}
				function envelope(
					sender = editor!,
					recipient = viewer!,
					epoch = 1,
					wrappedKey = randomBytes(80).toString('base64'),
				) {
					return {
						workspaceId: workspace,
						objectId: object,
						epoch,
						deviceId: recipient.id,
						wrappedKey,
						signingDevice: sender.id,
						signature: sign(
							null,
							Buffer.from(
								JSON.stringify([
									'noura.sync.key',
									1,
									workspace,
									object,
									epoch,
									sender.id,
									recipient.id,
									wrappedKey,
								]),
							),
							sender.f.keys.privateKey,
						).toString('base64'),
					};
				}
				const { storeOwnKey } = await import('./keys');
				const value = envelope();
				await storeOwnKey(store, editor!.actor, value, true);
				await storeOwnKey(store, editor!.actor, value, true);
				expect(
					await store.db`SELECT * FROM noura_key_envelopes WHERE workspace_id=${workspace}`,
				).toHaveLength(1);
				await expect(
					storeOwnKey(store, editor!.actor, envelope(), true),
				).rejects.toMatchObject({ code: 'sync.key_changed' });
				await expect(
					storeOwnKey(store, editor!.actor, value),
				).rejects.toMatchObject({ code: 'sync.forbidden' });
				await expect(
					storeOwnKey(store, viewer!.actor, envelope(viewer!, owner!), true),
				).rejects.toMatchObject({ code: 'sync.forbidden' });
				await expect(
					storeOwnKey(store, editor!.actor, envelope(editor!, outsider!), true),
				).rejects.toMatchObject({ code: 'sync.forbidden' });
				await expect(
					storeOwnKey(
						store,
						editor!.actor,
						envelope(editor!, viewer!, 2),
						true,
					),
				).rejects.toMatchObject({ code: 'sync.stale_epoch' });
				await expect(
					storeOwnKey(
						store,
						editor!.actor,
						{ ...value, deviceId: owner!.id },
						true,
					),
				).rejects.toMatchObject({ code: 'sync.invalid_signature' });
				await store.db`UPDATE noura_devices SET revoked=true WHERE id=${viewer!.id}`;
				await expect(
					storeOwnKey(store, editor!.actor, value, true),
				).rejects.toMatchObject({ code: 'sync.forbidden' });
				await store.db`UPDATE noura_devices SET revoked=true WHERE id=${editor!.id}`;
				await expect(
					storeOwnKey(store, editor!.actor, envelope(editor!, owner!), true),
				).rejects.toMatchObject({ code: 'sync.unauthorized' });
				const [state] =
					await store.db`SELECT access_revision FROM noura_workspaces WHERE id=${workspace}`;
				expect(String(state!.access_revision)).toBe('0');
				const members =
					await store.db`SELECT role FROM noura_members WHERE workspace_id=${workspace} ORDER BY role`;
				expect(members.map((member) => member.role)).toEqual([
					'editor',
					'owner',
					'viewer',
				]);
			} finally {
				await store.close();
			}
		});
	},
);
