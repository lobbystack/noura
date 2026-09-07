import { describe, expect, test } from 'bun:test';
import { SyncStore } from './store';
import { fixture } from './protocol.test';

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
	},
);
