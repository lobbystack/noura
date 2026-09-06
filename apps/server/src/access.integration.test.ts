import { describe, expect, test } from 'bun:test';
import { randomBytes, sign } from 'node:crypto';
import { SyncStore } from './store';
import {
	accessPolicy,
	accessDigest,
	accessSigningBytes,
	keySigningBytes,
	setAccess,
	type AccessPolicy,
} from './access';
import { fixture } from './protocol.test';
import { digest } from './protocol';
import { createApp } from './app';

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'signed access and atomic key epochs',
	() => {
		test('item sharing, key reads, revocation and stale writes enforce the same policy', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const suffix = crypto.randomUUID();
			const workspace = `w_${suffix}`;
			const owner = fixture(`owner_${suffix}`, workspace, 'a_object');
			const reader = fixture(`reader_${suffix}`, workspace, 'a_object');
			const ownerAccount = `a_${suffix}`;
			const readerAccount = `b_${suffix}`;
			const token = randomBytes(32).toString('base64url');
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${`owner_${suffix}`},${ownerAccount},${owner.publicKey}),(${`reader_${suffix}`},${readerAccount},${reader.publicKey})`;
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${`reader_${suffix}`},now()+interval '1 hour')`;
				const actor = {
					deviceId: `owner_${suffix}`,
					accountId: ownerAccount,
					publicKey: owner.publicKey,
				};
				const recipient = await store.authenticate(token);
				await store.createWorkspace(actor, workspace);
				await store.createObject(actor, workspace, 'a_object');
				await store.createObject(actor, workspace, 'b_private');
				const wrapped = new Map<string, string>();
				function policy(
					revision: string,
					share: boolean,
					epoch: number,
				): AccessPolicy {
					const members: AccessPolicy['members'] = [
						{ accountId: ownerAccount, role: 'owner' },
					];
					const objects: AccessPolicy['objects'] = [
						'a_object',
						'b_private',
					].map((objectId) => {
						const objectEpoch = objectId === 'a_object' ? epoch : 1;
						const devices = [
							actor.deviceId,
							...(share && objectId === 'a_object' ? [recipient.deviceId] : []),
						].sort();
						const envelopes = devices.map((deviceId) => {
							const id = `${objectId}:${objectEpoch}:${deviceId}`;
							if (!wrapped.has(id))
								wrapped.set(id, randomBytes(80).toString('base64'));
							const envelope = { deviceId, wrappedKey: wrapped.get(id)! };
							return {
								...envelope,
								signature: sign(
									null,
									keySigningBytes(
										workspace,
										objectId,
										objectEpoch,
										actor.deviceId,
										envelope,
									),
									owner.keys.privateKey,
								).toString('base64'),
							};
						});
						return {
							objectId,
							epoch: objectEpoch,
							grants:
								share && objectId === 'a_object'
									? [{ accountId: readerAccount, role: 'viewer' as const }]
									: [],
							envelopes,
						};
					});
					const previousPolicyDigest =
						revision === '1'
							? null
							: accessDigest(
									policy(
										String(Number(revision) - 1),
										Number(revision) - 1 === 1,
										Number(revision) - 1 === 1 ? 1 : 2,
									),
								);
					const value = {
						version: 1 as const,
						workspaceId: workspace,
						revision,
						previousPolicyDigest,
						deviceId: actor.deviceId,
						members,
						objects,
					};
					return accessPolicy({
						...value,
						signature: sign(
							null,
							accessSigningBytes(value),
							owner.keys.privateKey,
						).toString('base64'),
					});
				}
				const shared = policy('1', true, 1);
				await setAccess(store, actor, shared);
				await setAccess(store, actor, shared); // exact retry remains idempotent
				const wrongChainBody = {
					...policy('2', false, 2),
					previousPolicyDigest: '0'.repeat(64),
				};
				const wrongChain = accessPolicy({
					...wrongChainBody,
					signature: sign(
						null,
						accessSigningBytes(wrongChainBody),
						owner.keys.privateKey,
					).toString('base64'),
				});
				await expect(setAccess(store, actor, wrongChain)).rejects.toThrow(
					'sync.policy_chain_changed',
				);
				await store.push(actor, workspace, [owner.make(undefined, 1, '1')]);
				const privateFixture = fixture(actor.deviceId, workspace, 'b_private');
				// Owner's real signing key must authenticate every operation.
				const privateOp = {
					...privateFixture.make(undefined, 1, '1'),
					deviceId: actor.deviceId,
				};
				const { signingBytes } = await import('./protocol');
				privateOp.signature = sign(
					null,
					signingBytes(privateOp),
					owner.keys.privateKey,
				).toString('base64');
				await store.push(actor, workspace, [privateOp]);
				expect(
					(await store.pull(recipient, workspace, '0')).operations,
				).toHaveLength(1);
				expect(await store.createObject(recipient, workspace, 'a_object')).toBe(
					1,
				);
				const app = createApp(store, { origin: 'http://localhost:1900' });
				const keys = await (
					await app.request(`/v1/workspaces/${workspace}/keys`, {
						headers: { Authorization: `Bearer ${token}` },
					})
				).json();
				expect(keys.envelopes).toHaveLength(1);
				expect(keys.envelopes[0].objectId).toBe('a_object');
				expect(JSON.stringify(keys)).not.toContain('b_private');
				await expect(
					store.push(recipient, workspace, [reader.make()]),
				).rejects.toThrow('sync.forbidden');
				await expect(
					setAccess(store, actor, policy('2', false, 1)),
				).rejects.toThrow('sync.key_rotation_required');
				expect(
					(await store.pull(recipient, workspace, '0')).operations,
				).toHaveLength(1);
				await setAccess(store, actor, policy('2', false, 2));
				await expect(store.pull(recipient, workspace, '0')).rejects.toThrow(
					'sync.forbidden',
				);
				await expect(
					store.push(actor, workspace, [owner.make()]),
				).rejects.toThrow('sync.stale_epoch');
				await store.push(actor, workspace, [owner.make(undefined, 2, '2')]);
				const revokedKeys = await (
					await app.request(`/v1/workspaces/${workspace}/keys`, {
						headers: { Authorization: `Bearer ${token}` },
					})
				).json();
				expect(revokedKeys.envelopes).toHaveLength(0);
				await expect(setAccess(store, actor, shared)).rejects.toThrow(
					'sync.policy_revision_changed',
				);
				const tampered = {
					...policy('3', false, 2),
					members: [{ accountId: readerAccount, role: 'owner' as const }],
				};
				await expect(setAccess(store, actor, tampered)).rejects.toThrow(
					'sync.invalid_signature',
				);
			} finally {
				await store.close();
			}
		});
	},
);
