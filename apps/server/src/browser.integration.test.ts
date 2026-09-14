import { describe, expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { createApp } from './app';
import { browserKeySigningBytes } from './browser';
import { storeOwnKey } from './keys';
import { digest } from './protocol';
import { SyncStore } from './store';

function ed25519() {
	const keys = generateKeyPairSync('ed25519');
	return {
		keys,
		publicKey: keys.publicKey
			.export({ format: 'der', type: 'spki' })
			.subarray(-32)
			.toString('base64'),
	};
}

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'browser device key delivery',
	() => {
		test('stores and returns a browser envelope only for the enrolled recipient', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			store.rateLimit = async () => {};
			const workspace = `browser_${crypto.randomUUID()}`;
			const object = 'browser_object';
			const ownerId = `owner_${crypto.randomUUID()}`;
			const readerId = `reader_${crypto.randomUUID()}`;
			const owner = ed25519();
			const reader = ed25519();
			const recipientBytes = randomBytes(32);
			const recipient = `x25519:${recipientBytes.toString('base64')}`;
			function envelope(
				recipientPublicKey = recipientBytes.toString('base64'),
			) {
				const fields = {
					workspaceId: workspace,
					objectId: object,
					epoch: 1,
					signingDevice: ownerId,
					deviceId: readerId,
					recipientPublicKey,
					ephemeralPublicKey: randomBytes(32).toString('base64'),
					salt: randomBytes(32).toString('base64'),
					nonce: randomBytes(12).toString('base64'),
					wrappedKey: randomBytes(80).toString('base64'),
				};
				return {
					...fields,
					construction: 'web' as const,
					signature: sign(
						null,
						browserKeySigningBytes(fields),
						owner.keys.privateKey,
					).toString('base64'),
				};
			}
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key,encryption_recipient) VALUES(${readerId},${readerId},${reader.publicKey},${recipient})`;
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${ownerId},${ownerId},${owner.publicKey})`;
				const actor = {
					deviceId: ownerId,
					accountId: ownerId,
					publicKey: owner.publicKey,
				};
				await store.createWorkspace(actor, workspace);
				await store.createObject(actor, workspace, object);
				await store.db`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${readerId},'editor')`;
				const value = envelope();
				await storeOwnKey(store, actor, value, true);
				await storeOwnKey(store, actor, value, true);
				const rows =
					await store.db`SELECT construction,recipient_public_key,ephemeral_public_key,salt,nonce FROM noura_key_envelopes WHERE workspace_id=${workspace}`;
				expect(rows).toHaveLength(1);
				expect(rows[0]!.construction).toBe('web');
				expect(rows[0]!.recipient_public_key).toBe(value.recipientPublicKey);
				await expect(
					storeOwnKey(
						store,
						actor,
						envelope(randomBytes(32).toString('base64')),
						true,
					),
				).rejects.toMatchObject({ code: 'sync.invalid_recipient' });
				const token = randomBytes(32).toString('base64url');
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${readerId},now()+interval '1 hour')`;
				const app = createApp(store, { origin: 'http://localhost:1900' });
				const response = await app.request(`/v1/workspaces/${workspace}/keys`, {
					headers: { Authorization: `Bearer ${token}` },
				});
				const body = (await response.json()) as {
					envelopes: Array<Record<string, unknown>>;
				};
				expect(body.envelopes[0]).toMatchObject({
					construction: 'web',
					recipientPublicKey: value.recipientPublicKey,
					ephemeralPublicKey: value.ephemeralPublicKey,
					salt: value.salt,
					nonce: value.nonce,
					signingPublicKey: owner.publicKey,
				});
			} finally {
				await store.close();
			}
		});
	},
);
