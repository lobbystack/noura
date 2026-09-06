import { randomBytes } from 'node:crypto';
import type { EncryptedOperation } from '../../../packages/shared/src/sync';
import {
	digest,
	operationDigest,
	SyncError,
	verifyOperation,
} from './protocol';
import type { Actor, SyncStore } from './store';

export async function createPublicLink(
	store: SyncStore,
	actor: Actor,
	snapshot: EncryptedOperation,
	expiresInDays = 30,
) {
	if (
		!Number.isInteger(expiresInDays) ||
		expiresInDays < 1 ||
		expiresInDays > 30
	)
		throw new SyncError('sync.invalid_expiry');
	if (snapshot.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyOperation(snapshot, actor.publicKey);
	const token = randomBytes(32).toString('base64url');
	const id = `link_${crypto.randomUUID()}`;
	await store.withWorkspace(
		actor,
		snapshot.workspaceId,
		async (tx, state, role) => {
			if (!['owner', 'admin'].includes(role ?? ''))
				throw new SyncError('sync.forbidden', 403);
			const [object] =
				await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${snapshot.workspaceId} AND id=${snapshot.objectId}`;
			if (!object || Number(object.epoch) !== snapshot.epoch)
				throw new SyncError('sync.stale_epoch', 409);
			const bytes = Buffer.byteLength(snapshot.ciphertext, 'base64');
			if (BigInt(state.used_bytes) + BigInt(bytes) > BigInt(state.quota_bytes))
				throw new SyncError('sync.quota_exceeded', 413);
			await tx`INSERT INTO noura_public_links(id,token_hash,workspace_id,object_id,snapshot,payload_bytes,expires_at)
		 VALUES(${id},${digest(token)},${snapshot.workspaceId},${snapshot.objectId},${tx.json(JSON.parse(JSON.stringify(snapshot)))},${bytes},now()+${expiresInDays}*interval '1 day')`;
			await tx`UPDATE noura_workspaces SET used_bytes=used_bytes+${bytes} WHERE id=${snapshot.workspaceId}`;
		},
	);
	return { id, token, revision: '1', expiresInDays };
}

export async function updatePublicLink(
	store: SyncStore,
	actor: Actor,
	id: string,
	expected: string,
	snapshot: EncryptedOperation,
) {
	if (snapshot.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyOperation(snapshot, actor.publicKey);
	return await store.withWorkspace(
		actor,
		snapshot.workspaceId,
		async (tx, state, role) => {
			if (!['owner', 'admin'].includes(role ?? ''))
				throw new SyncError('sync.forbidden', 403);
			const [link] =
				await tx`SELECT * FROM noura_public_links WHERE id=${id} AND workspace_id=${snapshot.workspaceId} AND object_id=${snapshot.objectId} AND NOT revoked AND expires_at>now()`;
			if (!link) throw new SyncError('sync.not_found', 404);
			if (
				operationDigest(link.snapshot as EncryptedOperation) ===
				operationDigest(snapshot)
			)
				return { revision: String(link.revision) };
			if (String(link.revision) !== expected)
				throw new SyncError('sync.link_revision_changed', 409);
			const [object] =
				await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${snapshot.workspaceId} AND id=${snapshot.objectId}`;
			if (Number(object!.epoch) !== snapshot.epoch)
				throw new SyncError('sync.stale_epoch', 409);
			const bytes = Buffer.byteLength(snapshot.ciphertext, 'base64');
			const nextUsed =
				BigInt(state.used_bytes) - BigInt(link.payload_bytes) + BigInt(bytes);
			if (nextUsed > BigInt(state.quota_bytes))
				throw new SyncError('sync.quota_exceeded', 413);
			const revision = (BigInt(link.revision) + 1n).toString();
			await tx`UPDATE noura_public_links SET snapshot=${tx.json(JSON.parse(JSON.stringify(snapshot)))},revision=${revision},payload_bytes=${bytes} WHERE id=${id}`;
			await tx`UPDATE noura_workspaces SET used_bytes=${nextUsed.toString()} WHERE id=${snapshot.workspaceId}`;
			return { revision };
		},
	);
}

export async function revokePublicLink(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	id: string,
) {
	await store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		if (!['owner', 'admin'].includes(role ?? ''))
			throw new SyncError('sync.forbidden', 403);
		const [link] =
			await tx`UPDATE noura_public_links SET revoked=true WHERE id=${id} AND workspace_id=${workspace} RETURNING id`;
		if (!link) throw new SyncError('sync.not_found', 404);
	});
}

export async function readPublicLink(store: SyncStore, token: string) {
	if (!/^[A-Za-z0-9_-]{43}$/.test(token))
		throw new SyncError('sync.not_found', 404);
	const [link] =
		await store.db`SELECT snapshot,revision FROM noura_public_links WHERE token_hash=${digest(token)} AND NOT revoked AND expires_at>now()`;
	if (!link) throw new SyncError('sync.not_found', 404);
	return {
		snapshot: link.snapshot as EncryptedOperation,
		revision: String(link.revision),
	};
}
