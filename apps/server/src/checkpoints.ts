import { createPublicKey, verify } from 'node:crypto';
import { accessDigest, accessPolicy } from './access';
import {
	base64,
	cursor,
	digest,
	identifier,
	operation,
	operationDigest,
	record,
	SyncError,
	verifyOperation,
} from './protocol';
import type { Actor, SyncStore } from './store';
import { verifyWorkspaceCapability, workspaceCapability } from './capabilities';

import type { AccessTransition } from '../../../packages/shared/src/generated/AccessTransition';
import type { EncryptedCheckpoint } from '../../../packages/shared/src/generated/EncryptedCheckpoint';
export type { AccessTransition, EncryptedCheckpoint };

function exact(input: unknown, fields: string[]) {
	const value = record(input);
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_transition');
	return value;
}

export function checkpoint(input: unknown): EncryptedCheckpoint {
	const value = exact(input, [
		'version',
		'generation',
		'coveredSequence',
		'payload',
		'signature',
	]);
	if (value.version !== 1) throw new SyncError('sync.unsupported_version');
	return {
		version: 1,
		generation: identifier(value.generation),
		coveredSequence: cursor(value.coveredSequence),
		payload: operation(value.payload),
		signature: base64(value.signature, 64).toString('base64'),
	};
}

export function checkpointSigningBytes(value: EncryptedCheckpoint) {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.checkpoint',
			value.version,
			value.generation,
			value.coveredSequence,
			operationDigest(value.payload),
		]),
	);
}
export function checkpointDigest(value: EncryptedCheckpoint) {
	return digest(
		Buffer.concat([checkpointSigningBytes(value), base64(value.signature, 64)]),
	);
}
export function transitionSigningBytes(value: AccessTransition) {
	const tuple: unknown[] = [
		'noura.sync.transition',
		value.version,
		value.transitionId,
		value.coveredSequence,
		accessDigest(value.policy),
		value.checkpoints.map(checkpointDigest),
	];
	if (value.version === 2)
		tuple.push(
			(value.blobs ?? []).map((blob) => [
				blob.objectId,
				blob.epoch,
				blob.ciphertextDigest,
				blob.ciphertextSize,
			]),
		);
	return Buffer.from(JSON.stringify(tuple));
}
export function transitionDigest(value: AccessTransition) {
	return digest(
		Buffer.concat([transitionSigningBytes(value), base64(value.signature, 64)]),
	);
}
function verifySignature(bytes: Buffer, signature: string, publicKey: string) {
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			base64(publicKey, 32),
		]),
		format: 'der',
		type: 'spki',
	});
	if (!verify(null, bytes, key, base64(signature, 64)))
		throw new SyncError('sync.invalid_signature', 403);
}
export function verifyCheckpoint(
	value: EncryptedCheckpoint,
	publicKey: string,
) {
	verifyOperation(value.payload, publicKey);
	verifySignature(checkpointSigningBytes(value), value.signature, publicKey);
}
export function accessTransition(input: unknown): AccessTransition {
	const raw = record(input);
	if (raw.version !== 1 && raw.version !== 2)
		throw new SyncError('sync.unsupported_version');
	const value = exact(input, [
		'version',
		'transitionId',
		'coveredSequence',
		'policy',
		'checkpoints',
		...(raw.version === 2 ? ['blobs'] : []),
		'signature',
	]);
	if (!Array.isArray(value.checkpoints) || value.checkpoints.length > 1000)
		throw new SyncError('sync.invalid_transition');
	if (
		value.version === 2 &&
		(!Array.isArray(value.blobs) || value.blobs.length > 1000)
	)
		throw new SyncError('sync.invalid_transition');
	const blobs =
		value.version === 2
			? (value.blobs as unknown[]).map((input) => {
					const blob = exact(input, [
						'objectId',
						'epoch',
						'ciphertextDigest',
						'ciphertextSize',
					]);
					if (
						!Number.isSafeInteger(blob.epoch) ||
						(blob.epoch as number) < 1 ||
						typeof blob.ciphertextDigest !== 'string' ||
						!/^[0-9a-f]{64}$/.test(blob.ciphertextDigest) ||
						!Number.isSafeInteger(blob.ciphertextSize) ||
						(blob.ciphertextSize as number) < 1 ||
						(blob.ciphertextSize as number) > 1024 * 1024 * 1024
					)
						throw new SyncError('sync.invalid_transition_blob');
					return {
						objectId: identifier(blob.objectId),
						epoch: blob.epoch as number,
						ciphertextDigest: blob.ciphertextDigest,
						ciphertextSize: blob.ciphertextSize as number,
					};
				})
			: [];
	const result: AccessTransition = {
		version: value.version as 1 | 2,
		transitionId: identifier(value.transitionId),
		coveredSequence: cursor(value.coveredSequence),
		policy: accessPolicy(value.policy),
		checkpoints: value.checkpoints.map(checkpoint),
		...(value.version === 2 ? { blobs } : {}),
		signature: base64(value.signature, 64).toString('base64'),
	};
	let previous = '';
	for (const entry of result.checkpoints) {
		const op = entry.payload;
		const object = result.policy.objects.find(
			(object) => object.objectId === op.objectId,
		);
		if (
			op.objectId <= previous ||
			!object ||
			object.epoch !== op.epoch ||
			(object.document && object.document.generation !== entry.generation) ||
			op.workspaceId !== result.policy.workspaceId ||
			op.deviceId !== result.policy.deviceId ||
			op.policyRevision !== result.policy.revision ||
			entry.coveredSequence !== result.coveredSequence
		)
			throw new SyncError('sync.invalid_transition');
		previous = op.objectId;
	}
	let previousBlob = '';
	for (const blob of result.blobs ?? []) {
		const key = `${blob.objectId}\0${blob.ciphertextDigest}`;
		const object = result.policy.objects.find(
			(object) => object.objectId === blob.objectId,
		);
		if (
			key <= previousBlob ||
			!object ||
			object.epoch !== blob.epoch ||
			!result.checkpoints.some(
				(checkpoint) => checkpoint.payload.objectId === blob.objectId,
			)
		)
			throw new SyncError('sync.invalid_transition_blob');
		previousBlob = key;
	}
	return result;
}
export function verifyTransition(value: AccessTransition, publicKey: string) {
	for (const entry of value.checkpoints) {
		verifyCheckpoint(entry, publicKey);
	}
	verifySignature(transitionSigningBytes(value), value.signature, publicKey);
}

/** Staging reserves quota, but changes neither grants nor readable epochs. Retained for recovery. */
export async function stageTransition(
	store: SyncStore,
	actor: Actor,
	value: AccessTransition,
) {
	if (value.policy.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyTransition(value, actor.publicKey);
	const { verifyAccess } = await import('./access');
	verifyAccess(value.policy, actor.publicKey);
	return store.withWorkspace(
		actor,
		value.policy.workspaceId,
		async (tx, state, role) => {
			if (!['owner', 'admin'].includes(role ?? ''))
				throw new SyncError('sync.forbidden', 403);
			const [capability] =
				await tx`SELECT c.capability,d.public_key FROM noura_workspace_capabilities c JOIN noura_devices d ON d.id=c.device_id WHERE c.workspace_id=${value.policy.workspaceId}`;
			if (!capability)
				throw new SyncError('sync.collaboration_capability_required', 409);
			const signedCapability = workspaceCapability(capability.capability);
			if (signedCapability.workspaceId !== value.policy.workspaceId)
				throw new SyncError('sync.identity_mismatch', 403);
			verifyWorkspaceCapability(signedCapability, capability.public_key);
			const hash = transitionDigest(value);
			const [existing] =
				await tx`SELECT digest,committed FROM noura_transitions WHERE workspace_id=${value.policy.workspaceId} AND id=${value.transitionId}`;
			if (existing) {
				if (existing.digest !== hash)
					throw new SyncError('sync.transition_id_reused', 409);
				return {
					transitionId: value.transitionId,
					committed: existing.committed as boolean,
					digest: hash,
				};
			}
			if (
				BigInt(value.policy.revision) !== BigInt(state.access_revision) + 1n ||
				value.coveredSequence !== String(state.sequence)
			)
				throw new SyncError('sync.transition_stale', 409);
			const bytes = Buffer.byteLength(JSON.stringify(value));
			if (BigInt(state.used_bytes) + BigInt(bytes) > BigInt(state.quota_bytes))
				throw new SyncError('sync.quota_exceeded', 413);
			await tx`INSERT INTO noura_transitions(workspace_id,id,device_id,digest,body,payload_bytes) VALUES(${value.policy.workspaceId},${value.transitionId},${actor.deviceId},${hash},${tx.json(JSON.parse(JSON.stringify(value)))},${bytes})`;
			await tx`UPDATE noura_workspaces SET used_bytes=used_bytes+${bytes} WHERE id=${value.policy.workspaceId}`;
			return {
				transitionId: value.transitionId,
				committed: false,
				digest: hash,
			};
		},
	);
}

export async function readTransition(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	id: string,
) {
	return store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		const [row] =
			await tx`SELECT device_id,digest,committed FROM noura_transitions WHERE workspace_id=${workspace} AND id=${id}`;
		// The submitting device can resolve an uncertain commit even after removing itself.
		if (
			!row ||
			(row.device_id !== actor.deviceId &&
				!['owner', 'admin'].includes(role ?? ''))
		)
			throw new SyncError('sync.not_found', 404);
		return {
			transitionId: id,
			committed: row.committed as boolean,
			digest: row.digest as string,
		};
	});
}
