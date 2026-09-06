import { describe, expect, test } from 'bun:test';
import { SyncStore } from './store';
import { fixture } from './protocol.test';
import {
	createPublicLink,
	readPublicLink,
	revokePublicLink,
	updatePublicLink,
} from './public-links';

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'revocable encrypted public snapshots',
	() => {
		test('anonymous reads expose one encrypted snapshot, updates compare revisions, expiry and revocation close access', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const suffix = crypto.randomUUID();
			const workspace = `public_${suffix}`;
			const owner = fixture(`owner_${suffix}`, workspace, 'object');
			const actor = {
				deviceId: `owner_${suffix}`,
				accountId: `account_${suffix}`,
				publicKey: owner.publicKey,
			};
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${actor.deviceId},${actor.accountId},${actor.publicKey})`;
				await store.createWorkspace(actor, workspace);
				await store.createObject(actor, workspace, 'object');
				const snapshot = owner.make();
				const link = await createPublicLink(store, actor, snapshot);
				expect(link.expiresInDays).toBe(30);
				expect(await readPublicLink(store, link.token)).toEqual({
					snapshot,
					revision: '1',
				});
				const [row] =
					await store.db`SELECT token_hash FROM noura_public_links WHERE id=${link.id}`;
				expect(row!.token_hash).not.toBe(link.token);
				const next = owner.make();
				expect(
					await updatePublicLink(store, actor, link.id, '1', next),
				).toEqual({ revision: '2' });
				expect(
					await updatePublicLink(store, actor, link.id, '1', next),
				).toEqual({ revision: '2' });
				await expect(
					updatePublicLink(store, actor, link.id, '1', owner.make()),
				).rejects.toThrow('sync.link_revision_changed');
				await revokePublicLink(store, actor, workspace, link.id);
				await expect(readPublicLink(store, link.token)).rejects.toThrow(
					'sync.not_found',
				);
				await expect(
					updatePublicLink(store, actor, link.id, '2', owner.make()),
				).rejects.toThrow('sync.not_found');
				const expired = await createPublicLink(store, actor, owner.make(), 1);
				await store.db`UPDATE noura_public_links SET expires_at=now()-interval '1 second' WHERE id=${expired.id}`;
				await expect(readPublicLink(store, expired.token)).rejects.toThrow(
					'sync.not_found',
				);
				await expect(
					createPublicLink(store, actor, owner.make(), 31),
				).rejects.toThrow('sync.invalid_expiry');
			} finally {
				await store.close();
			}
		});
	},
);
