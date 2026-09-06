import { createHash, createPublicKey, verify } from 'node:crypto';
import { base64, cursor, identifier, record, SyncError } from './protocol';
import type { SyncStore, Actor } from './store';

export interface AccessPolicy {
	version: 1;
	workspaceId: string;
	revision: string;
	previousPolicyDigest: string | null;
	deviceId: string;
	members: {
		accountId: string;
		role: 'owner' | 'admin' | 'editor' | 'viewer';
	}[];
	objects: {
		objectId: string;
		epoch: number;
		grants: { accountId: string; role: 'editor' | 'viewer' }[];
		envelopes: { deviceId: string; wrappedKey: string; signature: string }[];
	}[];
	signature: string;
}

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
	if (body.version !== 1 || cursor(body.revision) === '0')
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
			const value = exact(input, ['objectId', 'epoch', 'grants', 'envelopes']);
			if (!Number.isSafeInteger(value.epoch) || (value.epoch as number) < 1)
				throw new SyncError('sync.invalid_epoch');
			return {
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
				envelopes: sorted(
					value.envelopes,
					(v: { deviceId: string; wrappedKey: string; signature: string }) =>
						v.deviceId,
					(input) => {
						const envelope = exact(input, [
							'deviceId',
							'wrappedKey',
							'signature',
						]);
						base64(envelope.wrappedKey, 60, 4096);
						base64(envelope.signature, 64);
						return {
							deviceId: identifier(envelope.deviceId),
							wrappedKey: envelope.wrappedKey as string,
							signature: envelope.signature as string,
						};
					},
				),
			};
		},
	);
	base64(body.signature, 64);
	return {
		version: 1,
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
				object.envelopes.map((envelope) => [
					envelope.deviceId,
					envelope.wrappedKey,
					envelope.signature,
				]),
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
			if (
				!verify(
					null,
					keySigningBytes(
						policy.workspaceId,
						object.objectId,
						object.epoch,
						policy.deviceId,
						envelope,
					),
					key,
					base64(envelope.signature, 64),
				)
			)
				throw new SyncError('sync.invalid_signature', 403);
		}
}

/** ACL changes and encrypted key rotation use the same lock as operation writes. */
export async function setAccess(
	store: SyncStore,
	actor: Actor,
	policy: AccessPolicy,
) {
	if (policy.deviceId !== actor.deviceId)
		throw new SyncError('sync.identity_mismatch', 403);
	verifyAccess(policy, actor.publicKey);
	await store.withWorkspace(
		actor,
		policy.workspaceId,
		async (tx, state, role) => {
			const workspace = policy.workspaceId;
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
			const expectedPrevious = previous
				? accessDigest(accessPolicy(previous.policy))
				: null;
			if (policy.previousPolicyDigest !== expectedPrevious)
				throw new SyncError('sync.policy_chain_changed', 409);
			const oldMembers =
				await tx`SELECT account_id,role FROM noura_members WHERE workspace_id=${workspace} ORDER BY account_id COLLATE "C"`;
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
				await tx`SELECT id,account_id FROM noura_devices WHERE account_id=ANY(${accounts}) AND NOT revoked ORDER BY id FOR SHARE`;
			const devices = new Map(
				activeDevices.map((device) => [device.id, device.account_id as string]),
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
					await tx`SELECT account_id,role FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				const oldAccounts = new Set(
					[...oldMembers, ...oldGrants].map(
						(value) => value.account_id as string,
					),
				);
				const nextAccounts = new Set(
					[...policy.members, ...next.grants].map((value) => value.accountId),
				);
				const oldEnvelopes =
					await tx`SELECT device_id,wrapped_key FROM noura_key_envelopes WHERE workspace_id=${workspace} AND object_id=${object.id} AND epoch=${object.epoch}`;
				const removesAccess =
					[...oldAccounts].some((account) => !nextAccounts.has(account)) ||
					oldEnvelopes.some((envelope) => !devices.has(envelope.device_id));
				if (
					next.epoch < Number(object.epoch) ||
					next.epoch > Number(object.epoch) + 1 ||
					(removesAccess && next.epoch !== Number(object.epoch) + 1)
				)
					throw new SyncError('sync.key_rotation_required', 409);
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
					const existing = oldEnvelopes.find(
						(old) => old.device_id === envelope.deviceId,
					);
					if (
						next.epoch === Number(object.epoch) &&
						existing &&
						existing.wrapped_key !== envelope.wrappedKey
					)
						throw new SyncError('sync.key_epoch_immutable', 409);
					await tx`INSERT INTO noura_key_envelopes(workspace_id,object_id,epoch,device_id,wrapped_key,signing_device,signature)
				 VALUES(${workspace},${object.id},${next.epoch},${envelope.deviceId},${envelope.wrappedKey},${actor.deviceId},${envelope.signature}) ON CONFLICT DO NOTHING`;
				}
				await tx`UPDATE noura_objects SET epoch=${next.epoch} WHERE workspace_id=${workspace} AND id=${object.id}`;
				if (next.epoch !== Number(object.epoch))
					await tx`UPDATE noura_public_links SET revoked=true WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				await tx`DELETE FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object.id}`;
				for (const grant of next.grants)
					await tx`INSERT INTO noura_grants(workspace_id,object_id,account_id,role) VALUES(${workspace},${object.id},${grant.accountId},${grant.role})`;
			}
			await tx`DELETE FROM noura_members WHERE workspace_id=${workspace}`;
			for (const member of policy.members)
				await tx`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${member.accountId},${member.role})`;
			for (const member of policy.members)
				await tx`UPDATE noura_invitations SET completed_at=now()
				 WHERE workspace_id=${workspace} AND accepted_account_id=${member.accountId}
				 AND role=${member.role} AND completed_at IS NULL AND revoked_at IS NULL`;
			await tx`INSERT INTO noura_access_log(workspace_id,revision,device_id,policy,signature) VALUES(${workspace},${policy.revision},${actor.deviceId},${tx.json(JSON.parse(JSON.stringify(policy)))},${policy.signature})`;
			await tx`UPDATE noura_workspaces SET access_revision=${policy.revision} WHERE id=${workspace}`;
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
