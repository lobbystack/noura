import { describe, expect, test } from 'bun:test';
import { sign } from 'node:crypto';
import { accessPolicy, accessSigningBytes, setAccess } from './access';
import {
	acceptInvitation,
	createInvitation,
	invitationDetails,
	listInvitations,
	revokeInvitation,
} from './invitations';
import { fixture } from './protocol.test';
import { SyncStore } from './store';

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'workspace invitations',
	() => {
		test('acceptance exposes candidate devices but grants access only through a signed policy', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const suffix = crypto.randomUUID();
			const workspace = `w_${suffix}`;
			const ownerAccount = `owner_account_${suffix}`;
			const guestAccount = `guest_account_${suffix}`;
			const outsiderAccount = `outsider_account_${suffix}`;
			const owner = fixture(`owner_device_${suffix}`, workspace, 'object');
			const guest = fixture(`guest_device_${suffix}`, workspace, 'object');
			const actor = {
				deviceId: `owner_device_${suffix}`,
				accountId: ownerAccount,
				publicKey: owner.publicKey,
			};
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key,encryption_recipient)
				 VALUES(${actor.deviceId},${ownerAccount},${owner.publicKey},${`age1${'q'.repeat(58)}`}),
				 (${`guest_device_${suffix}`},${guestAccount},${guest.publicKey},${`age1${'p'.repeat(58)}`})`;
				await store.createWorkspace(actor, workspace);

				const invitation = await createInvitation(
					store,
					actor,
					workspace,
					'viewer',
				);
				expect(invitation.token).toHaveLength(43);
				expect(await invitationDetails(store, invitation.token)).toMatchObject({
					workspaceId: workspace,
					role: 'viewer',
					accepted: false,
				});
				await acceptInvitation(store, invitation.token, guestAccount);
				await acceptInvitation(store, invitation.token, guestAccount);
				await expect(
					acceptInvitation(store, invitation.token, outsiderAccount),
				).rejects.toMatchObject({ code: 'sync.invitation_already_accepted' });
				const accepted = await listInvitations(store, actor, workspace);
				expect(accepted.invitations[0]).toMatchObject({
					id: invitation.id,
					accountId: guestAccount,
					status: 'accepted',
					devices: [{ deviceId: `guest_device_${suffix}` }],
				});
				await expect(
					store.pull(
						{
							deviceId: `guest_device_${suffix}`,
							accountId: guestAccount,
							publicKey: guest.publicKey,
						},
						workspace,
						'0',
					),
				).rejects.toMatchObject({ code: 'sync.forbidden' });

				const unsigned = {
					version: 1 as const,
					workspaceId: workspace,
					revision: '1',
					previousPolicyDigest: null,
					deviceId: actor.deviceId,
					members: [
						{ accountId: guestAccount, role: 'viewer' as const },
						{ accountId: ownerAccount, role: 'owner' as const },
					],
					objects: [],
				};
				await setAccess(
					store,
					actor,
					accessPolicy({
						...unsigned,
						signature: sign(
							null,
							accessSigningBytes(unsigned),
							owner.keys.privateKey,
						).toString('base64'),
					}),
				);
				expect(
					(await listInvitations(store, actor, workspace)).invitations[0],
				).toMatchObject({ status: 'completed' });
				await expect(
					invitationDetails(store, invitation.token),
				).rejects.toMatchObject({ code: 'sync.invitation_unavailable' });

				const revoked = await createInvitation(
					store,
					actor,
					workspace,
					'editor',
				);
				await revokeInvitation(store, actor, workspace, revoked.id);
				await expect(
					invitationDetails(store, revoked.token),
				).rejects.toMatchObject({ code: 'sync.invitation_unavailable' });
				await expect(
					createInvitation(store, actor, workspace, 'owner'),
				).rejects.toMatchObject({ code: 'sync.invalid_role' });
				await store.db`INSERT INTO noura_invitations
				 (id,token_hash,workspace_id,inviter_account_id,role,expires_at)
				 SELECT ${workspace} || '_limit_' || n, ${workspace} || '_hash_' || n,
				 ${workspace},${ownerAccount},'viewer',now()+interval '1 day'
				 FROM generate_series(1,100) AS n`;
				expect(
					(await listInvitations(store, actor, workspace)).invitations,
				).toHaveLength(100);
				await expect(
					createInvitation(store, actor, workspace, 'viewer'),
				).rejects.toMatchObject({ code: 'sync.invitation_limit' });
				await revokeInvitation(store, actor, workspace, `${workspace}_limit_1`);
				const replacement = await createInvitation(
					store,
					actor,
					workspace,
					'viewer',
				);
				expect(
					(await listInvitations(store, actor, workspace)).invitations.some(
						(item) => item.id === replacement.id,
					),
				).toBe(true);
			} finally {
				await store.close();
			}
		});
	},
);
