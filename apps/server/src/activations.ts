import { createPublicKey, verify } from 'node:crypto';
import type postgres from 'postgres';
import type { ObjectActivation } from '../../../packages/shared/src/generated/ObjectActivation';
import type { PolicyEnvelope } from '../../../packages/shared/src/generated/PolicyEnvelope';
import { keySigningBytes } from './access';
import {
	capabilityDigest,
	verifyWorkspaceCapability,
	workspaceCapability,
} from './capabilities';
import { checkpoint, checkpointDigest, verifyCheckpoint } from './checkpoints';
import {
	base64,
	cursor,
	digest,
	identifier,
	record,
	SyncError,
} from './protocol';
import type { WorkspaceCapability } from './capabilities';
import type { Actor, SyncStore } from './store';

type Tx = postgres.TransactionSql;

function exact(input: unknown, fields: string[]) {
	const value = record(input);
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_object_activation');
	return value;
}

function sorted<T>(
	input: unknown,
	key: (value: T) => string,
	parse: (value: unknown) => T,
) {
	if (!Array.isArray(input) || input.length > 1000)
		throw new SyncError('sync.invalid_object_activation');
	const values = input.map(parse);
	for (let index = 1; index < values.length; index++)
		if (key(values[index - 1]!) >= key(values[index]!))
			throw new SyncError('sync.invalid_object_activation');
	return values;
}

export function objectActivation(input: unknown): ObjectActivation {
	const raw = record(input);
	if (raw.version !== 1 && raw.version !== 2)
		throw new SyncError('sync.unsupported_version');
	const value = exact(input, [
		'version',
		'activationId',
		'workspaceId',
		'policyRevision',
		'coveredSequence',
		'capabilityDigest',
		'deviceId',
		'document',
		'envelopes',
		'checkpoint',
		...(raw.version === 2 ? ['blobs'] : []),
		'signature',
	]);
	const descriptor = exact(value.document, ['generation', 'mode']);
	if (!['text', 'attachment'].includes(descriptor.mode as string))
		throw new SyncError('sync.invalid_document_mode');
	const envelopes = sorted<PolicyEnvelope>(
		value.envelopes,
		(envelope) => envelope.deviceId,
		(input) => {
			const envelope = exact(input, ['deviceId', 'wrappedKey', 'signature']);
			base64(envelope.wrappedKey, 60, 4096);
			base64(envelope.signature, 64);
			return {
				deviceId: identifier(envelope.deviceId),
				wrappedKey: envelope.wrappedKey as string,
				signature: envelope.signature as string,
			};
		},
	);
	if (!envelopes.length) throw new SyncError('sync.invalid_object_activation');
	const blobs =
		raw.version === 2
			? sorted<NonNullable<ObjectActivation['blobs']>[number]>(
					value.blobs,
					(blob) => `${blob.objectId}\0${blob.ciphertextDigest}`,
					(input) => {
						const blob = exact(input, [
							'objectId',
							'epoch',
							'ciphertextDigest',
							'ciphertextSize',
						]);
						if (
							blob.epoch !== 1 ||
							typeof blob.ciphertextDigest !== 'string' ||
							!/^[0-9a-f]{64}$/.test(blob.ciphertextDigest) ||
							!Number.isSafeInteger(blob.ciphertextSize) ||
							(blob.ciphertextSize as number) < 1 ||
							(blob.ciphertextSize as number) > 1024 * 1024 * 1024
						)
							throw new SyncError('sync.invalid_object_activation_blob');
						return {
							objectId: identifier(blob.objectId),
							epoch: 1,
							ciphertextDigest: blob.ciphertextDigest,
							ciphertextSize: blob.ciphertextSize as number,
						};
					},
				)
			: [];
	if (blobs.length > 1)
		throw new SyncError('sync.invalid_object_activation_blob');
	if (
		typeof value.capabilityDigest !== 'string' ||
		!/^[0-9a-f]{64}$/.test(value.capabilityDigest)
	)
		throw new SyncError('sync.invalid_object_activation');
	const result: ObjectActivation = {
		version: raw.version as 1 | 2,
		activationId: identifier(value.activationId),
		workspaceId: identifier(value.workspaceId),
		policyRevision: cursor(value.policyRevision),
		coveredSequence: cursor(value.coveredSequence),
		capabilityDigest: value.capabilityDigest,
		deviceId: identifier(value.deviceId),
		document: {
			generation: identifier(descriptor.generation),
			mode: descriptor.mode as 'text' | 'attachment',
		},
		envelopes,
		checkpoint: checkpoint(value.checkpoint),
		...(raw.version === 2 ? { blobs } : {}),
		signature: base64(value.signature, 64).toString('base64'),
	};
	const operation = result.checkpoint.payload;
	if (
		operation.workspaceId !== result.workspaceId ||
		operation.deviceId !== result.deviceId ||
		operation.policyRevision !== result.policyRevision ||
		operation.epoch !== 1 ||
		result.checkpoint.coveredSequence !== result.coveredSequence ||
		result.checkpoint.generation !== result.document.generation ||
		(result.blobs ?? []).some(
			(blob) => blob.objectId !== operation.objectId || blob.epoch !== 1,
		)
	)
		throw new SyncError('sync.invalid_object_activation');
	return result;
}

export function activationSigningBytes(value: ObjectActivation) {
	const tuple: unknown[] = [
		'noura.sync.object-activation',
		value.version,
		value.activationId,
		value.workspaceId,
		value.policyRevision,
		value.coveredSequence,
		value.capabilityDigest,
		value.deviceId,
		[value.document.generation, value.document.mode],
		value.envelopes.map((envelope) => [
			envelope.deviceId,
			envelope.wrappedKey,
			envelope.signature,
		]),
		checkpointDigest(value.checkpoint),
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

export function activationDigest(value: ObjectActivation) {
	return digest(
		Buffer.concat([activationSigningBytes(value), base64(value.signature, 64)]),
	);
}

export function verifyObjectActivation(
	value: ObjectActivation,
	publicKey: string,
	capability: WorkspaceCapability,
) {
	if (value.capabilityDigest !== capabilityDigest(capability))
		throw new SyncError('sync.collaboration_capability_changed', 409);
	verifyCheckpoint(value.checkpoint, publicKey);
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			base64(publicKey, 32),
		]),
		format: 'der',
		type: 'spki',
	});
	for (const envelope of value.envelopes)
		if (
			!verify(
				null,
				keySigningBytes(
					value.workspaceId,
					value.checkpoint.payload.objectId,
					1,
					value.deviceId,
					envelope,
				),
				key,
				base64(envelope.signature, 64),
			)
		)
			throw new SyncError('sync.invalid_signature', 403);
	if (
		!verify(
			null,
			activationSigningBytes(value),
			key,
			base64(value.signature, 64),
		)
	)
		throw new SyncError('sync.invalid_signature', 403);
}

async function storedCapability(tx: Tx, workspace: string) {
	const [row] =
		await tx`SELECT c.capability,d.public_key FROM noura_workspace_capabilities c JOIN noura_devices d ON d.id=c.device_id WHERE c.workspace_id=${workspace}`;
	if (!row) throw new SyncError('sync.collaboration_capability_required', 409);
	const capability = workspaceCapability(row.capability);
	verifyWorkspaceCapability(capability, row.public_key);
	return capability;
}

async function validateCurrent(
	tx: Tx,
	state: postgres.Row,
	actor: Actor,
	value: ObjectActivation,
	role: string | undefined,
) {
	if (!role || role === 'viewer') throw new SyncError('sync.forbidden', 403);
	if (
		value.policyRevision !== String(state.access_revision) ||
		value.coveredSequence !== String(state.sequence)
	)
		throw new SyncError('sync.activation_stale', 409);
	const [latest] =
		await tx`SELECT policy FROM noura_access_log WHERE workspace_id=${value.workspaceId} AND revision=${value.policyRevision}`;
	if (latest?.policy?.version !== 2)
		throw new SyncError('sync.collaboration_capability_required', 409);
	const capability = await storedCapability(tx, value.workspaceId);
	verifyObjectActivation(value, actor.publicKey, capability);
	const objectId = value.checkpoint.payload.objectId;
	const [existing] =
		await tx`SELECT 1 FROM noura_objects WHERE workspace_id=${value.workspaceId} AND id=${objectId}`;
	if (existing) throw new SyncError('sync.object_already_exists', 409);
	const devices =
		await tx`SELECT d.id FROM noura_devices d JOIN noura_members m ON m.account_id=d.account_id AND m.workspace_id=${value.workspaceId} WHERE NOT d.revoked ORDER BY d.id COLLATE "C"`;
	const expected = devices.map((device) => device.id as string);
	if (
		JSON.stringify(expected) !==
		JSON.stringify(value.envelopes.map((envelope) => envelope.deviceId))
	)
		throw new SyncError('sync.key_envelopes_incomplete', 409);
	return objectId;
}

/** Stage exact bytes before any blob upload or visible object mutation. */
export async function stageObjectActivation(
	store: SyncStore,
	actor: Actor,
	input: unknown,
) {
	const value = objectActivation(input);
	if (value.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	return store.withWorkspace(
		actor,
		value.workspaceId,
		async (tx, state, role) => {
			const hash = activationDigest(value);
			const [existing] =
				await tx`SELECT digest,committed FROM noura_object_activations WHERE workspace_id=${value.workspaceId} AND id=${value.activationId}`;
			if (existing) {
				if (existing.digest !== hash)
					throw new SyncError('sync.activation_id_reused', 409);
				return {
					activationId: value.activationId,
					committed: existing.committed as boolean,
					digest: hash,
				};
			}
			const objectId = await validateCurrent(tx, state, actor, value, role);
			const bytes = Buffer.byteLength(JSON.stringify(value));
			if (BigInt(state.used_bytes) + BigInt(bytes) > BigInt(state.quota_bytes))
				throw new SyncError('sync.quota_exceeded', 413);
			await tx`INSERT INTO noura_object_activations(workspace_id,id,object_id,device_id,digest,body,payload_bytes) VALUES(${value.workspaceId},${value.activationId},${objectId},${actor.deviceId},${hash},${tx.json(JSON.parse(JSON.stringify(value)))},${bytes})`;
			await tx`UPDATE noura_workspaces SET used_bytes=used_bytes+${bytes} WHERE id=${value.workspaceId}`;
			return {
				activationId: value.activationId,
				committed: false,
				digest: hash,
			};
		},
	);
}

export async function readObjectActivation(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	id: string,
) {
	return store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		const [row] =
			await tx`SELECT device_id,digest,committed FROM noura_object_activations WHERE workspace_id=${workspace} AND id=${id}`;
		if (!row || (row.device_id !== actor.deviceId && !role))
			throw new SyncError('sync.not_found', 404);
		return {
			activationId: id,
			committed: row.committed as boolean,
			digest: row.digest as string,
		};
	});
}

export async function commitObjectActivation(
	store: SyncStore,
	actor: Actor,
	workspace: string,
	id: string,
) {
	return store.withWorkspace(actor, workspace, async (tx, state, role) => {
		const [row] =
			await tx`SELECT device_id,digest,body,committed FROM noura_object_activations WHERE workspace_id=${workspace} AND id=${id}`;
		if (!row || row.device_id !== actor.deviceId)
			throw new SyncError('sync.not_found', 404);
		if (row.committed)
			return {
				activationId: id,
				committed: true,
				digest: row.digest as string,
			};
		const value = objectActivation(row.body);
		if (
			value.workspaceId !== workspace ||
			activationDigest(value) !== row.digest
		)
			throw new SyncError('sync.invalid_object_activation');
		const objectId = await validateCurrent(tx, state, actor, value, role);
		for (const blob of value.blobs ?? []) {
			const [stored] =
				await tx`SELECT epoch,size,complete,failed,activation_id FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${objectId} AND id=${blob.ciphertextDigest}`;
			if (
				!stored ||
				Number(stored.epoch) !== 1 ||
				Number(stored.size) !== blob.ciphertextSize ||
				stored.activation_id !== id ||
				!stored.complete ||
				stored.failed
			)
				throw new SyncError('sync.activation_blob_incomplete', 409);
		}
		await tx`INSERT INTO noura_objects(workspace_id,id,epoch,generation,document_mode) VALUES(${workspace},${objectId},1,${value.document.generation},${value.document.mode})`;
		for (const envelope of value.envelopes)
			await tx`INSERT INTO noura_key_envelopes(workspace_id,object_id,epoch,device_id,wrapped_key,signing_device,signature) VALUES(${workspace},${objectId},1,${envelope.deviceId},${envelope.wrappedKey},${actor.deviceId},${envelope.signature})`;
		await tx`INSERT INTO noura_activation_checkpoints(workspace_id,object_id,epoch,activation_id,generation,covered_sequence,checkpoint) VALUES(${workspace},${objectId},1,${id},${value.document.generation},${value.coveredSequence},${tx.json(JSON.parse(JSON.stringify(value.checkpoint)))})`;
		await tx`UPDATE noura_object_activations SET committed=true WHERE workspace_id=${workspace} AND id=${id}`;
		await tx`SELECT pg_notify('noura_sync',${workspace})`;
		return { activationId: id, committed: true, digest: row.digest as string };
	});
}
