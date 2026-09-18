import {
	browserRecipientMatches,
	parseKeyEnvelope,
	verifyKeyEnvelopeSignature,
} from './browser';
import { SyncError } from './protocol';
import type { Actor, SyncStore } from './store';

/** Store a signed recipient envelope without changing object authorization or key epoch. */
export async function storeOwnKey(
	store: SyncStore,
	actor: Actor,
	input: unknown,
	share = false,
) {
	const value = parseKeyEnvelope(input);
	if (
		(!share && value.deviceId !== actor.deviceId) ||
		value.signingDevice !== actor.deviceId
	)
		throw new SyncError('sync.forbidden', 403);
	if (!verifyKeyEnvelopeSignature(value, actor.publicKey))
		throw new SyncError('sync.invalid_signature');
	await store.withWorkspace(
		actor,
		value.workspaceId,
		async (tx, _state, role) => {
			const [grant] =
				await tx`SELECT role FROM noura_grants WHERE workspace_id=${value.workspaceId} AND object_id=${value.objectId} AND account_id=${actor.accountId}`;
			if ((!role || role === 'viewer') && grant?.role !== 'editor')
				throw new SyncError('sync.forbidden', 403);
			const [row] =
				await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${value.workspaceId} AND id=${value.objectId}`;
			if (!row || Number(row.epoch) !== value.epoch)
				throw new SyncError('sync.stale_epoch', 409);
			const [allowed] =
				await tx`SELECT d.id,d.encryption_recipient FROM noura_devices d WHERE d.id=${value.deviceId} AND NOT d.revoked AND (
            EXISTS(SELECT 1 FROM noura_members m WHERE m.workspace_id=${value.workspaceId} AND m.account_id=d.account_id)
            OR EXISTS(SELECT 1 FROM noura_grants g WHERE g.workspace_id=${value.workspaceId} AND g.object_id=${value.objectId} AND g.account_id=d.account_id))`;
			if (!allowed) throw new SyncError('sync.forbidden', 403);
			// A browser envelope must wrap to the recipient device's enrolled X25519 key.
			const recipientPublic = value.recipientPublicKey;
			if (
				value.construction === 'web' &&
				(!recipientPublic ||
					!browserRecipientMatches(
						allowed.encryption_recipient,
						recipientPublic,
					))
			)
				throw new SyncError('sync.invalid_recipient');
			const [existing] =
				await tx`SELECT wrapped_key,construction,recipient_public_key,ephemeral_public_key,salt,nonce FROM noura_key_envelopes WHERE workspace_id=${value.workspaceId} AND object_id=${value.objectId} AND epoch=${value.epoch} AND device_id=${value.deviceId}`;
			if (existing) {
				if (
					existing.wrapped_key !== value.wrappedKey ||
					existing.construction !== value.construction ||
					(existing.recipient_public_key ?? null) !==
						value.recipientPublicKey ||
					(existing.ephemeral_public_key ?? null) !==
						value.ephemeralPublicKey ||
					(existing.salt ?? null) !== value.salt ||
					(existing.nonce ?? null) !== value.nonce
				)
					throw new SyncError('sync.key_changed', 409);
				return;
			}
			const [checkpoint] =
				await tx`SELECT 1 FROM noura_checkpoints WHERE workspace_id=${value.workspaceId} AND object_id=${value.objectId} LIMIT 1`;
			if (checkpoint) throw new SyncError('sync.key_rotation_required', 409);
			await tx`INSERT INTO noura_key_envelopes(workspace_id,object_id,epoch,device_id,wrapped_key,signing_device,signature,construction,recipient_public_key,ephemeral_public_key,salt,nonce) VALUES(${value.workspaceId},${value.objectId},${value.epoch},${value.deviceId},${value.wrappedKey},${actor.deviceId},${value.signature},${value.construction},${value.recipientPublicKey},${value.ephemeralPublicKey},${value.salt},${value.nonce})`;
		},
	);
}
