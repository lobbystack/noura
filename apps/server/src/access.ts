import {
	accessTransition,
	transitionDigest,
	verifyTransition,
	type AccessTransition,
} from './checkpoints';
import { createHash, createPublicKey, verify } from 'node:crypto';
import { browserKeySigningBytes, browserRecipientMatches } from './browser';
import { base64, cursor, identifier, record, SyncError } from './protocol';
import type { SyncStore, Actor } from './store';

import type { AccessPolicy as GeneratedAccessPolicy } from '../../../packages/shared/src/generated/AccessPolicy';
import type { PolicyEnvelope } from '../../../packages/shared/src/generated/PolicyEnvelope';

/** A policy envelope extended with the optional browser construction fields. */
export type AccessPolicyEnvelope = PolicyEnvelope & {
	construction?: 'age' | 'web';
	recipientPublicKey?: string;
	ephemeralPublicKey?: string;
	salt?: string;
	nonce?: string;
};

/** Access policy with envelopes widened to carry browser construction fields. */
export type AccessPolicy = Omit<GeneratedAccessPolicy, 'objects'> & {
	objects: Array<
		Omit<GeneratedAccessPolicy['objects'][number], 'envelopes'> & {
			envelopes: AccessPolicyEnvelope[];
		}
	>;
};

function exact(value: unknown, fields: string[]) {
	const body = record(value);
	if (
		Object.keys(body).length !== fields.length ||
		fields.some((field) => !(field in body))
	)
		throw new SyncError('sync.invalid_policy');
	return body;
}
function sorted<T>(
	input: unknown,
	key: (value: T) => string,
	parse: (value: unknown) => T,
): T[] {
	if (!Array.isArray(input) || input.length > 1000)
		throw new SyncError('sync.invalid_policy');
	const values = input.map(parse);
	for (let i = 1; i < values.length; i++)
		if (key(values[i - 1]!) >= key(values[i]!))
			throw new SyncError('sync.policy_order');
	return values;
}

/**
 * Parse one policy envelope. Absent `construction` means `age`; `construction:
 * "web"` additionally requires the four browser byte fields. Unknown
 * constructions and mixed or missing fields are rejected.
 */
function policyEnvelope(input: unknown): AccessPolicyEnvelope {
	const raw = record(input);
	const construction = raw.construction ?? 'age';
	if (construction !== 'age' && construction !== 'web')
		throw new SyncError('sync.invalid_policy');
	if (construction === 'web') {
		const envelope = exact(raw, [
			'deviceId',
			'wrappedKey',
			'signature',
			'construction',
			'recipientPublicKey',
			'ephemeralPublicKey',
			'salt',
			'nonce',
		]);
		base64(envelope.wrappedKey, 16, 4096);
		base64(envelope.recipientPublicKey, 32);
		base64(envelope.ephemeralPublicKey, 32);
		base64(envelope.salt, 32);
		base64(envelope.nonce, 12);
		base64(envelope.signature, 64);
		return {
			deviceId: identifier(envelope.deviceId),
			wrappedKey: envelope.wrappedKey as string,
			signature: envelope.signature as string,
			construction: 'web',
			recipientPublicKey: envelope.recipientPublicKey as string,
			ephemeralPublicKey: envelope.ephemeralPublicKey as string,
			salt: envelope.salt as string,
			nonce: envelope.nonce as string,
		};
	}
	const envelope = exact(
		raw,
		raw.construction === undefined
			? ['deviceId', 'wrappedKey', 'signature']
			: ['deviceId', 'wrappedKey', 'signature', 'construction'],
	);
	base64(envelope.wrappedKey, 60, 4096);
	base64(envelope.signature, 64);
	return {
		deviceId: identifier(envelope.deviceId),
		wrappedKey: envelope.wrappedKey as string,
		signature: envelope.signature as string,
		construction: 'age',
	};
}

export function accessPolicy(input: unknown): AccessPolicy {
	const body = exact(input, [
		'version',
		'workspaceId',
		'revision',
		'previousPolicyDigest',
		'deviceId',
		'members',
		'objects',
		'signature',
	]);
	if (![1, 2].includes(body.version as number) || cursor(body.revision) === '0')
		throw new SyncError('sync.invalid_policy');
	if (
		body.previousPolicyDigest !== null &&
		(typeof body.previousPolicyDigest !== 'string' ||
			!/^[0-9a-f]{64}$/.test(body.previousPolicyDigest))
	)
		throw new SyncError('sync.invalid_policy');
	const members = sorted(
		body.members,
		(v: AccessPolicy['members'][number]) => v.accountId,
		(input) => {
			const value = exact(input, ['accountId', 'role']);
			if (
				!['owner', 'admin', 'editor', 'viewer'].includes(value.role as string)
			)
				throw new SyncError('sync.invalid_role');
			return {
				accountId: identifier(value.accountId),
				role: value.role as AccessPolicy['members'][number]['role'],
			};
		},
	);
	if (!members.some((member) => member.role === 'owner'))
		throw new SyncError('sync.owner_required');
	const objects = sorted(
		body.objects,
		(v: AccessPolicy['objects'][number]) => v.objectId,
		(input) => {
			const value = exact(input, [
				'objectId',
				'epoch',
				'grants',
				'envelopes',
				...(body.version === 2 ? ['document'] : []),
			]);
			let document: AccessPolicy['objects'][number]['document'];
			if (body.version === 2) {
				const descriptor = exact(value.document, ['generation', 'mode']);
				if (!['text', 'attachment'].includes(descriptor.mode as string))
					throw new SyncError('sync.invalid_document_mode');
				document = {
					generation: identifier(descriptor.generation),
					mode: descriptor.mode as 'text' | 'attachment',
				};
			}
			if (!Number.isSafeInteger(value.epoch) || (value.epoch as number) < 1)
				throw new SyncError('sync.invalid_epoch');
			return {
				...(document ? { document } : {}),
				objectId: identifier(value.objectId),
				epoch: value.epoch as number,
				grants: sorted(
					value.grants,
					(v: { accountId: string; role: 'editor' | 'viewer' }) => v.accountId,
					(input) => {
						const grant = exact(input, ['accountId', 'role']);
						if (!['editor', 'viewer'].includes(grant.role as string))
							throw new SyncError('sync.invalid_role');
						return {
							accountId: identifier(grant.accountId),
							role: grant.role as 'editor' | 'viewer',
						};
					},
				),
				envelopes: sorted<AccessPolicyEnvelope>(
					value.envelopes,
					(v) => v.deviceId,
					policyEnvelope,
				),
			};
		},
	);
	base64(body.signature, 64);
	return {
		version: body.version as 1 | 2,
		workspaceId: identifier(body.workspaceId),
		revision: cursor(body.revision),
		previousPolicyDigest: body.previousPolicyDigest as string | null,
		deviceId: identifier(body.deviceId),
		members,
		objects,
		signature: body.signature as string,
	};
}

export function accessSigningBytes(policy: Omit<AccessPolicy, 'signature'>) {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.access',
			policy.version,
			policy.workspaceId,
			policy.revision,
			policy.previousPolicyDigest,
			policy.deviceId,
			policy.members.map((member) => [member.accountId, member.role]),
			policy.objects.map((object) => [
				object.objectId,
				object.epoch,
				object.grants.map((grant) => [grant.accountId, grant.role]),
				object.envelopes.map((envelope) =>
					envelope.construction === 'web'
						? [
								envelope.deviceId,
								envelope.wrappedKey,
								envelope.signature,
								'web',
								envelope.recipientPublicKey,
								envelope.ephemeralPublicKey,
								envelope.salt,
								envelope.nonce,
							]
						: [envelope.deviceId, envelope.wrappedKey, envelope.signature],
				),
				...(policy.version === 2
					? [[object.document!.generation, object.document!.mode]]
					: []),
			]),
		]),
	);
}

export function accessDigest(policy: AccessPolicy) {
	return createHash('sha256')
		.update(accessSigningBytes(policy))
		.update(base64(policy.signature, 64))
		.digest('hex');
}

export function verifyAccess(policy: AccessPolicy, publicKey: string) {
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			base64(publicKey, 32),
		]),
		format: 'der',
		type: 'spki',
	});
	if (
		!verify(null, accessSigningBytes(policy), key, base64(policy.signature, 64))
	)
		throw new SyncError('sync.invalid_signature', 403);
	for (const object of policy.objects)
		for (const envelope of object.envelopes) {
			const message =
				envelope.construction === 'web'
					? browserKeySigningBytes({
							workspaceId: policy.workspaceId,
							objectId: object.objectId,
							epoch: object.epoch,
							signingDevice: policy.deviceId,
							deviceId: envelope.deviceId,
							recipientPublicKey: envelope.recipientPublicKey!,
							ephemeralPublicKey: envelope.ephemeralPublicKey!,
							salt: envelope.salt!,
							nonce: envelope.nonce!,
							wrappedKey: envelope.wrappedKey,
						})
					: keySigningBytes(
							policy.workspaceId,
							object.objectId,
							object.epoch,
							policy.deviceId,
							envelope,
						);
			if (!verify(null, message, key, base64(envelope.signature, 64)))
				throw new SyncError('sync.invalid_signature', 403);
		}
}

/** ACL changes and encrypted key rotation use the same lock as operation writes. */
export async function setAccess(
	store: SyncStore,
	actor: Actor,
	policy: AccessPolicy,
	transition?: AccessTransition,
) {
	if (policy.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyAccess(policy, actor.publicKey);
	if (transition) {
		transition = accessTransition(transition);
		verifyTransition(transition, actor.publicKey);
		if (accessDigest(transition.policy) !== accessDigest(policy))
			throw new SyncError('sync.invalid_transition');
	}
	await store.withWorkspace(
		actor,
		policy.workspaceId,
		async (tx, state, role) => {
			const workspace = policy.workspaceId;
			if (transition) {
				const [staged] =
					await tx`SELECT digest,committed FROM noura_transitions WHERE workspace_id=${workspace} AND id=${transition.transitionId}`;
				if (!staged || staged.digest !== transitionDigest(transition))
					throw new SyncError('sync.transition_not_staged', 409);
				if (staged.committed) return;
				if (transition.coveredSequence !== String(state.sequence))
					throw new SyncError('sync.transition_stale', 409);
				for (const blob of transition.blobs ?? []) {
					const [stored] =
						await tx`SELECT epoch,size,complete,failed,transition_id FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${blob.objectId} AND id=${blob.ciphertextDigest}`;
					if (
						!stored ||
						Number(stored.epoch) !== blob.epoch ||
						Number(stored.size) !== blob.ciphertextSize ||
						stored.transition_id !== transition.transitionId ||
						!stored.complete ||
						stored.failed
					)
						throw new SyncError('sync.transition_blob_incomplete', 409);
				}
			}
			if (!['owner', 'admin'].includes(role ?? ''))
				throw new SyncError('sync.forbidden', 403);
			if (BigInt(policy.revision) === BigInt(state.access_revision)) {
				const [prior] =
					await tx`SELECT policy,signature FROM noura_access_log WHERE workspace_id=${workspace} AND revision=${policy.revision}`;
				if (
					prior &&
					prior.signature === policy.signature &&
					accessSigningBytes(prior.policy as AccessPolicy).equals(
						accessSigningBytes(policy),
					)
				)
					return;
			}
			if (BigInt(policy.revision) !== BigInt(state.access_revision) + 1n)
				throw new SyncError('sync.policy_revision_changed', 409);
			const [previous] =
				await tx`SELECT policy FROM noura_access_log WHERE workspace_id=${workspace} ORDER BY revision DESC LIMIT 1`;
			if (
				(previous?.policy as AccessPolicy | undefined)?.version === 2 &&
				policy.version !== 2
			)
				throw new SyncError('sync.client_upgrade_required', 409);
			const expectedPrevious = previous
				? accessDigest(accessPolicy(previous.policy))
				: null;
			if (policy.previousPolicyDigest !== expectedPrevious)
				throw new SyncError('sync.policy_chain_changed', 409);
			const oldMembers =
				await tx`SELECT account_id,role,history_after FROM noura_members WHERE workspace_id=${workspace} ORDER BY account_id COLLATE "C"`;
			const [capabilityState] =
				await tx`SELECT 1 FROM noura_workspace_capabilities WHERE workspace_id=${workspace}`;
			const changedMembers =
				JSON.stringify(oldMembers.map((m) => [m.account_id, m.role])) !==
				JSON.stringify(policy.members.map((m) => [m.accountId, m.role]));
			if (changedMembers && role !== 'owner')
				throw new SyncError('sync.owner_required', 403);
			const objects =
				await tx`SELECT id,epoch FROM noura_objects WHERE workspace_id=${workspace} ORDER BY id COLLATE "C"`;
			if (
				objects.length !== policy.objects.length ||
				objects.some((object, i) => object.id !== policy.objects[i]!.objectId)
			)
				throw new SyncError('sync.policy_objects_changed', 409);
			const accounts = [
				...new Set([
					...oldMembers.map((m) => m.account_id as string),
					...policy.members.map((m) => m.accountId),
					...policy.objects.flatMap((o) => o.grants.map((g) => g.accountId)),
				]),
			];
			const activeDevices =
				await tx`SELECT id,account_id,encryption_recipient FROM noura_devices WHERE account_id=ANY(${accounts}) AND NOT revoked ORDER BY id FOR SHARE`;
			const devices = new Map(
				activeDevices.map((device) => [device.id, device.account_id as string]),
			);
			const deviceRecipients = new Map(
				activeDevices.map((device) => [
					device.id as string,
					device.encryption_recipient as string | null,
				]),
			);
			for (const account of new Set([
				...policy.members.map((m) => m.accountId),
				...policy.objects.flatMap((o) => o.grants.map((g) => g.accountId)),
			])) {
				if (!activeDevices.some((device) => device.account_id === account))
					throw new SyncError('sync.recipient_device_required', 409);
			}
			for (let i = 0; i < objects.length; i++) {
				const object = objects[i]!;
				const next = policy.objects[i]!;
				const oldGrants =
					await tx`SELECT account_id,role,history_after FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				const oldAccounts = new Set(
					[...oldMembers, ...oldGrants].map(
						(value) => value.account_id as string,
					),
				);
				const nextAccounts = new Set(
					[...policy.members, ...next.grants].map((value) => value.accountId),
				);
				const oldEnvelopes =
					await tx`SELECT device_id,wrapped_key,construction FROM noura_key_envelopes WHERE workspace_id=${workspace} AND object_id=${object.id} AND epoch=${object.epoch}`;
				const [checkpointState] =
					await tx`SELECT 1 FROM noura_checkpoints WHERE workspace_id=${workspace} AND object_id=${object.id} LIMIT 1`;
				const changesReaders =
					[...nextAccounts].some((account) => !oldAccounts.has(account)) ||
					next.envelopes.some(
						(envelope) =>
							!oldEnvelopes.some((old) => old.device_id === envelope.deviceId),
					);
				const removesAccess =
					[...oldAccounts].some((account) => !nextAccounts.has(account)) ||
					oldEnvelopes.some((envelope) => !devices.has(envelope.device_id));
				if (capabilityState && changesReaders && !transition)
					throw new SyncError('sync.access_transition_required', 409);
				if (
					next.epoch < Number(object.epoch) ||
					next.epoch > Number(object.epoch) + 1 ||
					((removesAccess ||
						((transition || checkpointState) && changesReaders)) &&
						next.epoch !== Number(object.epoch) + 1)
				)
					throw new SyncError('sync.key_rotation_required', 409);
				const rotates = next.epoch !== Number(object.epoch);
				const checkpoint = transition?.checkpoints.find(
					(entry) => entry.payload.objectId === object.id,
				);
				if ((transition || checkpointState) && rotates && !checkpoint)
					throw new SyncError('sync.checkpoint_required', 409);
				const priorObject = (
					previous?.policy as AccessPolicy | undefined
				)?.objects.find((entry) => entry.objectId === object.id);
				if (
					policy.version === 2 &&
					JSON.stringify(priorObject?.document) !==
						JSON.stringify(next.document) &&
					!checkpoint
				)
					throw new SyncError('sync.checkpoint_required', 409);
				if (
					checkpoint &&
					policy.version === 2 &&
					checkpoint.generation !== next.document?.generation
				)
					throw new SyncError('sync.invalid_generation');
				if (checkpoint && !rotates)
					throw new SyncError('sync.checkpoint_epoch_unchanged', 409);
				if (checkpoint) {
					const [reused] =
						await tx`SELECT 1 FROM noura_checkpoints WHERE workspace_id=${workspace} AND object_id=${object.id} AND generation=${checkpoint.generation}`;
					if (reused) throw new SyncError('sync.generation_reused', 409);
					await tx`INSERT INTO noura_checkpoints(workspace_id,object_id,epoch,transition_id,generation,covered_sequence,checkpoint) VALUES(${workspace},${object.id},${next.epoch},${transition!.transitionId},${checkpoint.generation},${checkpoint.coveredSequence},${tx.json(JSON.parse(JSON.stringify(checkpoint)))})`;
				}
				const expectedDevices = activeDevices
					.filter((device) => nextAccounts.has(device.account_id))
					.map((device) => device.id)
					.sort();
				if (
					JSON.stringify(expectedDevices) !==
					JSON.stringify(next.envelopes.map((envelope) => envelope.deviceId))
				)
					throw new SyncError('sync.key_envelopes_incomplete', 409);
				for (const envelope of next.envelopes) {
					if (
						envelope.construction === 'web' &&
						(!envelope.recipientPublicKey ||
							!browserRecipientMatches(
								deviceRecipients.get(envelope.deviceId),
								envelope.recipientPublicKey,
							))
					)
						throw new SyncError('sync.invalid_recipient');
					const existing = oldEnvelopes.find(
						(old) => old.device_id === envelope.deviceId,
					);
					if (
						next.epoch === Number(object.epoch) &&
						existing &&
						(existing.wrapped_key !== envelope.wrappedKey ||
							(existing.construction ?? 'age') !==
								(envelope.construction ?? 'age'))
					)
						throw new SyncError('sync.key_epoch_immutable', 409);
					await tx`INSERT INTO noura_key_envelopes(workspace_id,object_id,epoch,device_id,wrapped_key,signing_device,signature,construction,recipient_public_key,ephemeral_public_key,salt,nonce)
				 VALUES(${workspace},${object.id},${next.epoch},${envelope.deviceId},${envelope.wrappedKey},${actor.deviceId},${envelope.signature},${envelope.construction ?? 'age'},${envelope.recipientPublicKey ?? null},${envelope.ephemeralPublicKey ?? null},${envelope.salt ?? null},${envelope.nonce ?? null}) ON CONFLICT DO NOTHING`;
				}
				await tx`UPDATE noura_objects SET epoch=${next.epoch},generation=${next.document?.generation ?? null},document_mode=${next.document?.mode ?? null} WHERE workspace_id=${workspace} AND id=${object.id}`;
				if (next.epoch !== Number(object.epoch))
					await tx`UPDATE noura_public_links SET revoked=true WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				await tx`DELETE FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				for (const grant of next.grants) {
					const prior = oldGrants.find(
						(entry) => entry.account_id === grant.accountId,
					);
					const historyAfter =
						prior?.history_after ?? transition?.coveredSequence ?? '0';
					await tx`INSERT INTO noura_grants(workspace_id,object_id,account_id,role,history_after) VALUES(${workspace},${object.id},${grant.accountId},${grant.role},${historyAfter})`;
				}
			}
			await tx`DELETE FROM noura_members WHERE workspace_id=${workspace}`;
			for (const member of policy.members) {
				const prior = oldMembers.find(
					(entry) => entry.account_id === member.accountId,
				);
				const historyAfter =
					prior?.history_after ?? transition?.coveredSequence ?? '0';
				await tx`INSERT INTO noura_members(workspace_id,account_id,role,history_after) VALUES(${workspace},${member.accountId},${member.role},${historyAfter})`;
			}
			for (const member of policy.members)
				await tx`UPDATE noura_invitations SET completed_at=now()
				 WHERE workspace_id=${workspace} AND accepted_account_id=${member.accountId}
				 AND role=${member.role} AND completed_at IS NULL AND revoked_at IS NULL`;
			await tx`INSERT INTO noura_access_log(workspace_id,revision,device_id,policy,signature) VALUES(${workspace},${policy.revision},${actor.deviceId},${tx.json(JSON.parse(JSON.stringify(policy)))},${policy.signature})`;
			await tx`UPDATE noura_workspaces SET access_revision=${policy.revision} WHERE id=${workspace}`;
			if (transition)
				await tx`UPDATE noura_transitions SET committed=true WHERE workspace_id=${workspace} AND id=${transition.transitionId}`;
			await tx`SELECT pg_notify('noura_sync',${workspace})`;
		},
	);
}

export function keySigningBytes(
	workspace: string,
	object: string,
	epoch: number,
	signer: string,
	envelope: { deviceId: string; wrappedKey: string },
) {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.key',
			1,
			workspace,
			object,
			epoch,
			signer,
			envelope.deviceId,
			envelope.wrappedKey,
		]),
	);
}
