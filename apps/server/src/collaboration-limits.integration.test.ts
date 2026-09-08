import { describe, expect, test } from 'bun:test';
import { SyncStore } from './store';
import { fixture } from './protocol.test';
import type { EncryptedPresence } from '../../../packages/shared/src/generated/EncryptedPresence';

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'collaboration request budgets',
	() => {
		test('device and presence buckets are independent of account traffic and refill', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const f = fixture();
			const actor = {
				deviceId: crypto.randomUUID(),
				accountId: crypto.randomUUID(),
				publicKey: f.publicKey,
			};
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${actor.deviceId},${actor.accountId},${actor.publicKey})`;
				await store.collaborationRateLimit(actor);
				const [initial] =
					await store.db`SELECT tokens FROM noura_collaboration_limits WHERE device_id=${actor.deviceId} AND bucket='durable'`;
				expect(Number(initial!.tokens)).toBe(39);
				await store.db`UPDATE noura_collaboration_limits SET tokens=0,updated_at=clock_timestamp()+interval '1 hour' WHERE device_id=${actor.deviceId}`;
				await expect(store.collaborationRateLimit(actor)).rejects.toThrow(
					'sync.rate_limited',
				);
				await store.collaborationRateLimit(actor, 'presence');
				await store.rateLimit(actor);
				await store.db`UPDATE noura_collaboration_limits SET tokens=0,updated_at=clock_timestamp()-interval '1 second' WHERE device_id=${actor.deviceId} AND bucket='durable'`;
				await store.collaborationRateLimit(actor);
				const [refilled] =
					await store.db`SELECT tokens FROM noura_collaboration_limits WHERE device_id=${actor.deviceId} AND bucket='durable'`;
				expect(Number(refilled!.tokens)).toBeGreaterThanOrEqual(19);
				expect(Number(refilled!.tokens)).toBeLessThan(21);
				const second = { ...actor, deviceId: crypto.randomUUID() };
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${second.deviceId},${second.accountId},${second.publicKey})`;
				await store.collaborationRateLimit(second);
			} finally {
				await store.close();
			}
		});
		test('presence is transient, permission checked, and relayed across listeners', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const id = crypto.randomUUID();
			const f = fixture(`device_${id}`, `workspace_${id}`, `object_${id}`);
			const actor = {
				deviceId: `device_${id}`,
				accountId: `account_${id}`,
				publicKey: f.publicKey,
			};
			const controller = new AbortController();
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${actor.deviceId},${actor.accountId},${actor.publicKey})`;
				await store.createWorkspace(actor, `workspace_${id}`);
				await store.createObject(actor, `workspace_${id}`, `object_${id}`);
				await store.db`UPDATE noura_objects SET generation='generation' WHERE workspace_id=${`workspace_${id}`} AND id=${`object_${id}`}`;
				let receive!: (payload: string) => void;
				const received = new Promise<string>((resolve) => {
					receive = resolve;
				});
				await store.watchPresence(
					`workspace_${id}`,
					controller.signal,
					receive,
				);
				const value: EncryptedPresence = {
					version: 1,
					workspaceId: `workspace_${id}`,
					objectId: `object_${id}`,
					generation: 'generation',
					epoch: 1,
					deviceId: actor.deviceId,
					sessionId: `session_${id}`,
					sequence: 1,
					nonce: Buffer.alloc(12).toString('base64'),
					ciphertext: Buffer.alloc(16).toString('base64'),
					signature: Buffer.alloc(64).toString('base64'),
				};
				await store.publishPresence(actor, value);
				expect(JSON.parse(await received)).toMatchObject({
					type: 'presence',
					presence: value,
				});
				const tables =
					await store.db`SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename LIKE '%presence%'`;
				expect(tables).toHaveLength(0);
				await store.db`UPDATE noura_devices SET revoked=true WHERE id=${actor.deviceId}`;
				await expect(store.publishPresence(actor, value)).rejects.toThrow(
					'sync.unauthorized',
				);
			} finally {
				controller.abort();
				await store.close();
			}
		});
	},
);
