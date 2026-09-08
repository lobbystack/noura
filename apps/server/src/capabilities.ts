import { createPublicKey, verify } from 'node:crypto';
import type { WorkspaceCapability } from '../../../packages/shared/src/generated/WorkspaceCapability';
export type { WorkspaceCapability };
import { base64, digest, identifier, record, SyncError } from './protocol';
import type { Actor, SyncStore } from './store';

function exact(input: unknown, fields: string[]) {
	const value = record(input);
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_collaboration_capability');
	return value;
}

export function workspaceCapability(input: unknown): WorkspaceCapability {
	const value = exact(input, [
		'version',
		'workspaceId',
		'collaborationVersion',
		'minimumClientVersion',
		'minimumRelayVersion',
		'deviceId',
		'signature',
	]);
	if (
		value.version !== 1 ||
		value.collaborationVersion !== 1 ||
		value.minimumClientVersion !== 1 ||
		value.minimumRelayVersion !== 1
	)
		throw new SyncError('sync.incompatible_collaboration_capability', 409);
	return {
		version: 1,
		workspaceId: identifier(value.workspaceId),
		collaborationVersion: 1,
		minimumClientVersion: 1,
		minimumRelayVersion: 1,
		deviceId: identifier(value.deviceId),
		signature: base64(value.signature, 64).toString('base64'),
	};
}

export function capabilitySigningBytes(value: WorkspaceCapability) {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.workspace-capability',
			value.version,
			value.workspaceId,
			value.collaborationVersion,
			value.minimumClientVersion,
			value.minimumRelayVersion,
			value.deviceId,
		]),
	);
}

export function capabilityDigest(value: WorkspaceCapability) {
	return digest(
		Buffer.concat([capabilitySigningBytes(value), base64(value.signature, 64)]),
	);
}

export function verifyWorkspaceCapability(
	value: WorkspaceCapability,
	publicKey: string,
) {
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			base64(publicKey, 32),
		]),
		format: 'der',
		type: 'spki',
	});
	if (
		!verify(
			null,
			capabilitySigningBytes(value),
			key,
			base64(value.signature, 64),
		)
	)
		throw new SyncError('sync.invalid_signature', 403);
}

export async function putWorkspaceCapability(
	store: SyncStore,
	actor: Actor,
	input: unknown,
) {
	const value = workspaceCapability(input);
	if (value.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyWorkspaceCapability(value, actor.publicKey);
	return store.withWorkspace(
		actor,
		value.workspaceId,
		async (tx, _state, role) => {
			if (role !== 'owner') throw new SyncError('sync.owner_required', 403);
			const [existing] =
				await tx`SELECT capability FROM noura_workspace_capabilities WHERE workspace_id=${value.workspaceId}`;
			if (existing) {
				const prior = workspaceCapability(existing.capability);
				if (JSON.stringify(prior) !== JSON.stringify(value))
					throw new SyncError('sync.collaboration_capability_changed', 409);
				return prior;
			}
			await tx`INSERT INTO noura_workspace_capabilities(workspace_id,device_id,capability) VALUES(${value.workspaceId},${actor.deviceId},${tx.json(JSON.parse(JSON.stringify(value)))})`;
			return value;
		},
	);
}

export async function readWorkspaceCapability(
	store: SyncStore,
	actor: Actor,
	workspace: string,
) {
	return store.withWorkspace(actor, workspace, async (tx) => {
		const [row] =
			await tx`SELECT capability FROM noura_workspace_capabilities WHERE workspace_id=${workspace}`;
		if (!row) throw new SyncError('sync.not_found', 404);
		return workspaceCapability(row.capability);
	});
}
