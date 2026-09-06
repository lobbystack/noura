import { Hono } from 'hono';
import { bodyLimit } from 'hono/body-limit';
import { secureHeaders } from 'hono/secure-headers';
import { serveStatic } from 'hono/bun';
import { join } from 'node:path';
import { randomBytes, createPublicKey, verify } from 'node:crypto';
import type { AccountAuth } from './auth';
import { accessPolicy, setAccess } from './access';
import { storeOwnKey } from './keys';
import {
	acceptInvitation,
	createInvitation,
	invitationDetails,
	listInvitations,
	revokeInvitation,
} from './invitations';
import type { BlobService } from './blobs';
import {
	createPublicLink,
	updatePublicLink,
	revokePublicLink,
	readPublicLink,
} from './public-links';
import {
	base64,
	cursor,
	digest,
	identifier,
	operation,
	record,
	SyncError,
} from './protocol';
import { SyncStore, type Actor } from './store';

type Env = { Variables: { actor: Actor } };
export function createApp(
	store: SyncStore,
	options: {
		origin: string;
		auth?: AccountAuth;
		webRoot?: string;
		blobs?: BlobService;
	},
) {
	const app = new Hono<Env>();
	app.use('*', secureHeaders());
	app.use(
		'*',
		bodyLimit({
			maxSize: 2 * 1024 * 1024,
			onError: (c) =>
				c.json({ error: { code: 'sync.request_too_large' } }, 413),
		}),
	);
	app.use('*', async (c, next) => {
		c.header('Cache-Control', 'no-store');
		const origin = c.req.header('Origin');
		if (origin && origin !== options.origin)
			return c.json({ error: { code: 'sync.origin_forbidden' } }, 403);
		await next();
	});
	app.onError((error, c) => {
		if (error instanceof SyncError)
			return c.json({ error: { code: error.code } }, error.status as 400);
		if (error instanceof SyntaxError)
			return c.json({ error: { code: 'sync.invalid_json' } }, 400);
		if ('code' in error && error.code === '23505')
			return c.json({ error: { code: 'sync.already_exists' } }, 409);
		return c.json({ error: { code: 'sync.internal_error' } }, 500);
	});
	app.get('/health', (c) => c.json({ status: 'ok', protocol: 1 }));
	app.get('/ready', async (c) => {
		try {
			await store.ready();
			await options.blobs?.ready();
		} catch {
			return c.json({ status: 'unavailable' }, 503);
		}
		return c.json({ status: 'ready' });
	});
	if (options.auth) {
		const auth = options.auth;
		app.on(['GET', 'POST'], '/api/auth/*', (c) => auth.handler(c.req.raw));
		app.post('/v1/device-challenges', async (c) => {
			if (c.req.header('Origin') !== options.origin)
				throw new SyncError('sync.origin_forbidden', 403);
			const session = await auth.api.getSession({ headers: c.req.raw.headers });
			if (!session) throw new SyncError('sync.unauthorized', 401);
			const challenge = randomBytes(32).toString('base64url');
			await store.db.begin(async (tx) => {
				await tx`SELECT pg_advisory_xact_lock(hashtextextended(${session.user.id},0))`;
				await tx`DELETE FROM noura_device_challenges WHERE account_id=${session.user.id} OR expires_at<=now()`;
				await tx`INSERT INTO noura_device_challenges(token_hash,account_id,expires_at) VALUES(${digest(challenge)},${session.user.id},now()+interval '5 minutes')`;
			});
			return c.json({ challenge, accountId: session.user.id, expiresIn: 300 });
		});
		// Enrollment requires an authenticated account AND proof of the device signing key.
		app.post('/v1/devices', async (c) => {
			if (c.req.header('Origin') !== options.origin)
				throw new SyncError('sync.origin_forbidden', 403);
			const session = await auth.api.getSession({ headers: c.req.raw.headers });
			if (!session) throw new SyncError('sync.unauthorized', 401);
			const body = record(await c.req.json());
			const nonce = identifier(body.challenge);
			const deviceId = identifier(body.deviceId);
			const publicKey = base64(body.publicKey, 32);
			const proof = base64(body.proof, 64);
			const recipient = body.encryptionRecipient ?? null;
			if (
				recipient !== null &&
				(typeof recipient !== 'string' ||
					!/^age1[023456789acdefghjklmnpqrstuvwxyz]{58}$/.test(recipient))
			)
				throw new SyncError('sync.invalid_recipient');
			const key = createPublicKey({
				key: Buffer.concat([
					Buffer.from('302a300506032b6570032100', 'hex'),
					publicKey,
				]),
				format: 'der',
				type: 'spki',
			});
			const challenge = Buffer.from(
				JSON.stringify([
					recipient === null ? 'noura.device.enroll' : 'noura.device.enroll.v2',
					options.origin,
					session.user.id,
					deviceId,
					publicKey.toString('base64'),
					...(recipient === null ? [] : [recipient]),
					nonce,
				]),
			);
			if (!verify(null, challenge, key, proof))
				throw new SyncError('sync.invalid_signature', 403);
			const token = randomBytes(32).toString('base64url');
			await store.db.begin(async (tx) => {
				const [valid] =
					await tx`DELETE FROM noura_device_challenges WHERE token_hash=${digest(nonce)} AND account_id=${session.user.id} AND expires_at>now() RETURNING token_hash`;
				if (!valid) throw new SyncError('sync.invalid_challenge', 403);
				await tx`INSERT INTO noura_devices(id,account_id,public_key,encryption_recipient) VALUES(${deviceId},${session.user.id},${publicKey.toString('base64')},${recipient}) ON CONFLICT DO NOTHING`;
				const [device] =
					await tx`SELECT * FROM noura_devices WHERE id=${deviceId} FOR UPDATE`;
				if (
					!device ||
					device.account_id !== session.user.id ||
					device.public_key !== publicKey.toString('base64') ||
					device.encryption_recipient !== recipient ||
					device.revoked
				)
					throw new SyncError('sync.device_unavailable', 403);
				await tx`DELETE FROM noura_sessions WHERE device_id=${deviceId}`;
				await tx`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${deviceId},now()+interval '7 days')`;
			});
			return c.json({ deviceId, token, expiresIn: 604800 }, 201);
		});
		app.get('/api/invitations/:token', (c) =>
			invitationDetails(store, c.req.param('token')).then((value) =>
				c.json(value),
			),
		);
		app.post('/api/invitations/:token/accept', async (c) => {
			if (c.req.header('Origin') !== options.origin)
				throw new SyncError('sync.origin_forbidden', 403);
			const session = await auth.api.getSession({ headers: c.req.raw.headers });
			if (!session) throw new SyncError('sync.unauthorized', 401);
			return c.json(
				await acceptInvitation(store, c.req.param('token'), session.user.id),
			);
		});
	}
	app.use('/v1/*', async (c, next) => {
		const authorization = c.req.header('Authorization') ?? '';
		if (!/^Bearer [A-Za-z0-9_-]{43}$/.test(authorization))
			throw new SyncError('sync.unauthorized', 401);
		const actor = await store.authenticate(authorization.slice(7));
		await store.rateLimit(actor);
		c.set('actor', actor);
		await next();
	});
	app.get('/v1/workspaces', async (c) => {
		const rows =
			await store.db`SELECT workspace_id AS id,role FROM noura_members WHERE account_id=${c.get('actor').accountId} ORDER BY workspace_id`;
		return c.json({ workspaces: rows });
	});
	app.put('/v1/keys/self', async (c) => {
		await storeOwnKey(store, c.get('actor'), await c.req.json());
		return c.body(null, 204);
	});
	app.put('/v1/keys/share', async (c) => {
		const input = await c.req.json();
		if (!Array.isArray(input) || input.length === 0 || input.length > 1000)
			throw new SyncError('sync.invalid_key');
		for (const envelope of input)
			await storeOwnKey(store, c.get('actor'), envelope, true);
		return c.body(null, 204);
	});
	if (options.blobs) {
		const blobs = options.blobs;
		app.post('/v1/workspaces/:workspace/objects/:object/blobs', async (c) =>
			c.json(
				await blobs.create(
					c.get('actor'),
					c.req.param('workspace'),
					c.req.param('object'),
					await c.req.json(),
				),
				201,
			),
		);
		app.on(
			// Hono dispatches HEAD through its GET route while preserving req.raw.method.
			['GET', 'HEAD', 'PATCH', 'DELETE'],
			'/v1/workspaces/:workspace/objects/:object/blobs/:blob',
			(c) =>
				blobs.transfer(
					c.get('actor'),
					c.req.param('workspace'),
					c.req.param('object'),
					c.req.param('blob'),
					c.req.raw,
				),
		);
		app.get(
			'/v1/workspaces/:workspace/objects/:object/blobs/:blob/content',
			(c) =>
				blobs.read(
					c.get('actor'),
					c.req.param('workspace'),
					c.req.param('object'),
					c.req.param('blob'),
					c.req.header('Range'),
				),
		);
	}
	app.get('/v1/devices', async (c) => {
		const rows =
			await store.db`SELECT id,public_key AS "publicKey",encryption_recipient AS "encryptionRecipient",revoked FROM noura_devices WHERE account_id=${c.get('actor').accountId} ORDER BY id`;
		return c.json({ devices: rows });
	});
	app.delete('/v1/devices/:device', async (c) => {
		const device = identifier(c.req.param('device'));
		await store.db.begin(async (tx) => {
			const [row] =
				await tx`UPDATE noura_devices SET revoked=true WHERE id=${device} AND account_id=${c.get('actor').accountId} RETURNING id`;
			if (!row) throw new SyncError('sync.not_found', 404);
			await tx`DELETE FROM noura_sessions WHERE device_id=${device}`;
		});
		return c.body(null, 204);
	});
	app.post('/v1/workspaces', async (c) => {
		const body = record(await c.req.json());
		const id = identifier(body.id);
		await store.createWorkspace(c.get('actor'), id);
		return c.json({ id }, 201);
	});
	app.post('/v1/workspaces/:workspace/invitations', async (c) => {
		const workspace = identifier(c.req.param('workspace'));
		const body = record(await c.req.json());
		return c.json(
			await createInvitation(store, c.get('actor'), workspace, body.role),
			201,
		);
	});
	app.get('/v1/workspaces/:workspace/invitations', async (c) =>
		c.json(
			await listInvitations(
				store,
				c.get('actor'),
				identifier(c.req.param('workspace')),
			),
		),
	);
	app.delete('/v1/workspaces/:workspace/invitations/:invitation', async (c) => {
		await revokeInvitation(
			store,
			c.get('actor'),
			identifier(c.req.param('workspace')),
			identifier(c.req.param('invitation')),
		);
		return c.body(null, 204);
	});
	app.post('/v1/workspaces/:workspace/objects', async (c) => {
		const body = record(await c.req.json());
		const id = identifier(body.id);
		const epoch = await store.createObject(
			c.get('actor'),
			identifier(c.req.param('workspace')),
			id,
		);
		return c.json({ id, epoch }, 201);
	});
	app.post('/v1/workspaces/:workspace/operations', async (c) => {
		const body = record(await c.req.json());
		if (
			!Array.isArray(body.operations) ||
			body.operations.length < 1 ||
			body.operations.length > 100
		)
			throw new SyncError('sync.invalid_batch');
		const operations = body.operations.map(operation);
		const sequences = await store.push(
			c.get('actor'),
			identifier(c.req.param('workspace')),
			operations,
		);
		return c.json({ sequences });
	});
	app.get('/v1/workspaces/:workspace/operations', async (c) => {
		const workspace = identifier(c.req.param('workspace'));
		const after = cursor(c.req.query('after') ?? '0');
		const wait = c.req.query('wait');
		if (wait !== undefined && wait !== '25')
			throw new SyncError('sync.invalid_wait');
		const revision = c.req.query('accessRevision');
		if (revision !== undefined) cursor(revision);
		const watcher = wait
			? await store.watch(workspace, c.req.raw.signal)
			: undefined;
		try {
			const page = await store.pull(c.get('actor'), workspace, after);
			if (
				!watcher ||
				page.cursor !== after ||
				page.operations.length ||
				revision !== page.accessRevision
			)
				return c.json(page);
			await watcher.changed;
			// Long polling grants no extension to session lifetime or workspace access.
			const actor = await store.authenticate(
				c.req.header('Authorization')!.slice(7),
			);
			return c.json(await store.pull(actor, workspace, after));
		} finally {
			watcher?.close();
		}
	});
	app.put('/v1/workspaces/:workspace/access', async (c) => {
		const policy = accessPolicy(await c.req.json());
		if (policy.workspaceId !== identifier(c.req.param('workspace')))
			throw new SyncError('sync.identity_mismatch', 403);
		await setAccess(store, c.get('actor'), policy);
		return c.json({ revision: policy.revision });
	});
	app.get('/v1/workspaces/:workspace/keys', async (c) => {
		const workspace = identifier(c.req.param('workspace'));
		const actor = c.get('actor');
		const recipientDevice =
			c.req.query('device') === undefined
				? actor.deviceId
				: identifier(c.req.query('device'));
		const afterObject =
			c.req.query('afterObject') === undefined
				? ''
				: identifier(c.req.query('afterObject'));
		const afterEpoch = cursor(c.req.query('afterEpoch') ?? '0');
		const envelopes = await store.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				const [owned] =
					await tx`SELECT id FROM noura_devices WHERE id=${recipientDevice} AND account_id=${actor.accountId}`;
				if (!owned) throw new SyncError('sync.forbidden', 403);
				const rows =
					await tx`SELECT k.object_id AS "objectId",k.epoch::text,k.device_id AS "deviceId",k.wrapped_key AS "wrappedKey",k.signing_device AS "signingDevice",k.signature,d.public_key AS "signingPublicKey"
			 FROM noura_key_envelopes k JOIN noura_devices d ON d.id=k.signing_device
			 WHERE k.workspace_id=${workspace} AND k.device_id=${recipientDevice} AND k.signature IS NOT NULL
			 AND (k.object_id COLLATE "C",k.epoch)>(${afterObject},${afterEpoch}::bigint)
			 AND (${Boolean(role)} OR EXISTS(SELECT 1 FROM noura_grants g WHERE g.workspace_id=k.workspace_id AND g.object_id=k.object_id AND g.account_id=${actor.accountId}))
			 ORDER BY k.object_id COLLATE "C",k.epoch LIMIT 101`;
				return rows;
			},
		);
		return c.json({
			envelopes: envelopes.slice(0, 100),
			hasMore: envelopes.length > 100,
		});
	});
	app.get('/v1/workspaces/:workspace/access', async (c) => {
		const workspace = identifier(c.req.param('workspace'));
		const after = cursor(c.req.query('after') ?? '0');
		const result = await store.withWorkspace(
			c.get('actor'),
			workspace,
			async (tx, state, role) => {
				if (!role) throw new SyncError('sync.forbidden', 403);
				const policies =
					await tx`SELECT policy FROM noura_access_log WHERE workspace_id=${workspace} AND revision>${after} ORDER BY revision LIMIT 10`;
				return {
					revision: String(state.access_revision),
					policies: policies.map((row) => row.policy),
				};
			},
		);
		return c.json(result);
	});
	app.get('/v1/workspaces/:workspace/access-state', async (c) => {
		const workspace = identifier(c.req.param('workspace'));
		return c.json(
			await store.withWorkspace(
				c.get('actor'),
				workspace,
				async (tx, state, role) => {
					if (!role) throw new SyncError('sync.forbidden', 403);
					const members =
						await tx`SELECT account_id AS "accountId",role FROM noura_members WHERE workspace_id=${workspace} ORDER BY account_id COLLATE "C" LIMIT 1001`;
					const objects =
						await tx`SELECT id AS "objectId",epoch::text FROM noura_objects WHERE workspace_id=${workspace} ORDER BY id COLLATE "C" LIMIT 1001`;
					const envelopes =
						await tx`SELECT object_id AS "objectId",epoch::text,device_id AS "deviceId",wrapped_key AS "wrappedKey",signing_device AS "signingDevice",signature FROM noura_key_envelopes k WHERE workspace_id=${workspace} AND epoch=(SELECT epoch FROM noura_objects o WHERE o.workspace_id=k.workspace_id AND o.id=k.object_id) ORDER BY object_id COLLATE "C",epoch,device_id COLLATE "C" LIMIT 10001`;
					const devices =
						await tx`SELECT id AS "deviceId",account_id AS "accountId",public_key AS "publicKey",encryption_recipient AS "encryptionRecipient" FROM noura_devices WHERE NOT revoked AND account_id IN (SELECT account_id FROM noura_members WHERE workspace_id=${workspace} UNION SELECT account_id FROM noura_grants WHERE workspace_id=${workspace}) ORDER BY id COLLATE "C" LIMIT 1001`;
					if (
						members.length > 1000 ||
						objects.length > 1000 ||
						devices.length > 1000 ||
						envelopes.length > 10000
					)
						throw new SyncError('sync.access_state_limit', 413);
					const [latest] =
						await tx`SELECT policy FROM noura_access_log WHERE workspace_id=${workspace} ORDER BY revision DESC LIMIT 1`;
					return {
						revision: String(state.access_revision),
						members,
						objects,
						envelopes,
						devices,
						policy: latest?.policy ?? null,
					};
				},
			),
		);
	});
	if (options.webRoot) {
		app.get('/_app/*', serveStatic({ root: options.webRoot }));
		const index = async (c: import('hono').Context<Env>) => {
			c.header('Referrer-Policy', 'no-referrer');
			c.header('Content-Security-Policy', "frame-ancestors 'none'");
			const file = Bun.file(join(options.webRoot!, 'index.html'));
			if (!(await file.exists()))
				return c.json({ error: { code: 'server.web_unavailable' } }, 503);
			return c.html(await file.text());
		};
		app.get('/account', index);
		app.get('/account/device', index);
		app.get('/invite/:token', index);
		app.get('/share/:token', index);
	}
	app.get('/public/:token', async (c) =>
		c.json(await readPublicLink(store, c.req.param('token'))),
	);
	app.post('/v1/workspaces/:workspace/links', async (c) => {
		const body = record(await c.req.json());
		const snapshot = operation(body.snapshot);
		if (snapshot.workspaceId !== identifier(c.req.param('workspace')))
			throw new SyncError('sync.identity_mismatch', 403);
		return c.json(
			await createPublicLink(
				store,
				c.get('actor'),
				snapshot,
				body.expiresInDays === undefined ? 30 : (body.expiresInDays as number),
			),
			201,
		);
	});
	app.put('/v1/workspaces/:workspace/links/:link', async (c) => {
		const body = record(await c.req.json());
		const snapshot = operation(body.snapshot);
		if (snapshot.workspaceId !== identifier(c.req.param('workspace')))
			throw new SyncError('sync.identity_mismatch', 403);
		return c.json(
			await updatePublicLink(
				store,
				c.get('actor'),
				identifier(c.req.param('link')),
				cursor(body.expectedRevision),
				snapshot,
			),
		);
	});
	app.delete('/v1/workspaces/:workspace/links/:link', async (c) => {
		await revokePublicLink(
			store,
			c.get('actor'),
			identifier(c.req.param('workspace')),
			identifier(c.req.param('link')),
		);
		return c.body(null, 204);
	});
	return app;
}
