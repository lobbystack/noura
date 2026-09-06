import { randomBytes } from 'node:crypto';
import { digest, SyncError } from './protocol';
import type { Actor, SyncStore } from './store';

export type InvitationRole = 'admin' | 'editor' | 'viewer';

function invitationRole(value: unknown): InvitationRole {
	if (!['admin', 'editor', 'viewer'].includes(value as string))
		throw new SyncError('sync.invalid_role');
	return value as InvitationRole;
}

export async function createInvitation(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	roleInput: unknown,
): Promise<{
	id: string;
	workspaceId: string;
	role: InvitationRole;
	expiresAt: string | Date;
	token: string;
}> {
	const role = invitationRole(roleInput);
	const token = randomBytes(32).toString('base64url');
	const id = `invite_${randomBytes(16).toString('hex')}`;
	const row = await store.withWorkspace(
		actor,
		workspace,
		async (tx, _state, memberRole) => {
			if (memberRole !== 'owner') throw new SyncError('sync.forbidden', 403);
			const [active] =
				await tx`SELECT count(*)::int AS count FROM noura_invitations
			 WHERE workspace_id=${workspace} AND completed_at IS NULL
			 AND revoked_at IS NULL AND expires_at>now()`;
			if (Number(active?.count) >= 100)
				throw new SyncError('sync.invitation_limit', 413);
			const [created] = await tx`INSERT INTO noura_invitations
			 (id,token_hash,workspace_id,inviter_account_id,role,expires_at)
			 VALUES(${id},${digest(token)},${workspace},${actor.accountId},${role},now()+interval '7 days')
			 RETURNING id,workspace_id AS "workspaceId",role,expires_at AS "expiresAt"`;
			return created!;
		},
	);
	return {
		id: row.id as string,
		workspaceId: row.workspaceId as string,
		role: invitationRole(row.role),
		expiresAt: row.expiresAt as string | Date,
		token,
	};
}

export async function listInvitations(
	store: SyncStore,
	actor: Actor,
	workspace: string,
) {
	return store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		if (role !== 'owner') throw new SyncError('sync.forbidden', 403);
		const invitations =
			await tx`SELECT id,role,accepted_account_id AS "accountId",
		 expires_at AS "expiresAt",accepted_at AS "acceptedAt",completed_at AS "completedAt",
		 revoked_at AS "revokedAt",created_at AS "createdAt"
		 FROM noura_invitations WHERE workspace_id=${workspace}
		 ORDER BY (completed_at IS NULL AND revoked_at IS NULL AND expires_at>now()) DESC,
		 created_at DESC LIMIT 100`;
		const accounts = invitations
			.map((invite) => invite.accountId as string | null)
			.filter((account): account is string => Boolean(account));
		const devices = accounts.length
			? await tx`SELECT id AS "deviceId",account_id AS "accountId",public_key AS "publicKey",
			 encryption_recipient AS "encryptionRecipient"
			 FROM noura_devices WHERE account_id=ANY(${accounts}) AND NOT revoked
			 ORDER BY id COLLATE "C" LIMIT 1001`
			: [];
		if (devices.length > 1000)
			throw new SyncError('sync.invitation_device_limit', 413);
		return {
			invitations: invitations.map((invite) => ({
				id: invite.id,
				role: invite.role,
				accountId: invite.accountId,
				expiresAt: invite.expiresAt,
				status: invite.revokedAt
					? 'revoked'
					: invite.completedAt
						? 'completed'
						: new Date(invite.expiresAt as string | Date).getTime() <=
							  Date.now()
							? 'expired'
							: invite.acceptedAt
								? 'accepted'
								: 'pending',
				devices: devices.filter(
					(device) => device.accountId === invite.accountId,
				),
			})),
		};
	});
}

export async function invitationDetails(store: SyncStore, token: string) {
	if (!/^[A-Za-z0-9_-]{43}$/.test(token))
		throw new SyncError('sync.invitation_unavailable', 404);
	const [row] =
		await store.db`SELECT workspace_id AS "workspaceId",role,expires_at AS "expiresAt",
	 accepted_account_id AS "accountId",completed_at AS "completedAt",revoked_at AS "revokedAt"
	 FROM noura_invitations WHERE token_hash=${digest(token)}`;
	if (
		!row ||
		row.revokedAt ||
		row.completedAt ||
		new Date(row.expiresAt as string | Date).getTime() <= Date.now()
	)
		throw new SyncError('sync.invitation_unavailable', 404);
	return {
		workspaceId: row.workspaceId as string,
		role: invitationRole(row.role),
		accepted: Boolean(row.accountId),
		expiresAt: row.expiresAt,
	};
}

export async function acceptInvitation(
	store: SyncStore,
	token: string,
	accountId: string,
) {
	if (!/^[A-Za-z0-9_-]{43}$/.test(token))
		throw new SyncError('sync.invitation_unavailable', 404);
	return store.db.begin(async (tx) => {
		const [row] = await tx`SELECT * FROM noura_invitations
		 WHERE token_hash=${digest(token)} FOR UPDATE`;
		if (
			!row ||
			row.revoked_at ||
			row.completed_at ||
			new Date(row.expires_at as string | Date).getTime() <= Date.now()
		)
			throw new SyncError('sync.invitation_unavailable', 404);
		if (row.accepted_account_id && row.accepted_account_id !== accountId)
			throw new SyncError('sync.invitation_already_accepted', 409);
		await tx`UPDATE noura_invitations
		 SET accepted_account_id=${accountId},accepted_at=COALESCE(accepted_at,now())
		 WHERE id=${row.id}`;
		return { workspaceId: row.workspace_id, role: invitationRole(row.role) };
	});
}

export async function revokeInvitation(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	id: string,
) {
	await store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		if (role !== 'owner') throw new SyncError('sync.forbidden', 403);
		const [row] = await tx`UPDATE noura_invitations SET revoked_at=now()
		 WHERE id=${id} AND workspace_id=${workspace} AND completed_at IS NULL
		 AND revoked_at IS NULL RETURNING id`;
		if (!row) throw new SyncError('sync.not_found', 404);
	});
}
