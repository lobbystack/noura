import { createPublicKey, verify } from 'node:crypto';
import { base64, identifier, record, SyncError } from './protocol';
import type { Actor, SyncStore } from './store';

/** Store a signed recipient envelope without changing object authorization or key epoch. */
export async function storeOwnKey(
	store: SyncStore,
	actor: Actor,
	input: unknown,
	share = false,
) {
	const value = record(input);
	const fields = [
		'workspaceId',
		'objectId',
		'epoch',
		'deviceId',
		'wrappedKey',
		'signingDevice',
		'signature',
	];
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_key');
	const workspace = identifier(value.workspaceId);
	const object = identifier(value.objectId);
	if (
		(!share && value.deviceId !== actor.deviceId) ||
		value.signingDevice !== actor.deviceId
	)
		throw new SyncError('sync.forbidden', 403);
	if (!Number.isSafeInteger(value.epoch) || (value.epoch as number) < 1)
		throw new SyncError('sync.invalid_epoch');
	const epoch = value.epoch as number;
	const recipient = identifier(value.deviceId);
	base64(value.wrappedKey, 60, 4096);
	const signature = base64(value.signature, 64);
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			Buffer.from(actor.publicKey, 'base64'),
		]),
		format: 'der',
		type: 'spki',
	});
	if (
		!verify(
			null,
			Buffer.from(
				JSON.stringify([
					'noura.sync.key',
					1,
					workspace,
					object,
					epoch,
					actor.deviceId,
					recipient,
					value.wrappedKey,
				]),
			),
			key,
			signature,
		)
	)
		throw new SyncError('sync.invalid_signature');
	await store.withWorkspace(actor, workspace, async (tx, _state, role) => {
		const [grant] =
			await tx`SELECT role FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object} AND account_id=${actor.accountId}`;
		if ((!role || role === 'viewer') && grant?.role !== 'editor')
			throw new SyncError('sync.forbidden', 403);
		const [row] =
			await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${workspace} AND id=${object}`;
		if (!row || Number(row.epoch) !== epoch)
			throw new SyncError('sync.stale_epoch', 409);
		const [allowed] =
			await tx`SELECT d.id FROM noura_devices d WHERE d.id=${recipient} AND NOT d.revoked AND (
            EXISTS(SELECT 1 FROM noura_members m WHERE m.workspace_id=${workspace} AND m.account_id=d.account_id)
            OR EXISTS(SELECT 1 FROM noura_grants g WHERE g.workspace_id=${workspace} AND g.object_id=${object} AND g.account_id=d.account_id))`;
		if (!allowed) throw new SyncError('sync.forbidden', 403);
		const [existing] =
			await tx`SELECT wrapped_key FROM noura_key_envelopes WHERE workspace_id=${workspace} AND object_id=${object} AND epoch=${epoch} AND device_id=${recipient}`;
		if (existing) {
			if (existing.wrapped_key !== value.wrappedKey)
				throw new SyncError('sync.key_changed', 409);
			return;
		}
		await tx`INSERT INTO noura_key_envelopes(workspace_id,object_id,epoch,device_id,wrapped_key,signing_device,signature) VALUES(${workspace},${object},${epoch},${recipient},${value.wrappedKey as string},${actor.deviceId},${value.signature as string})`;
	});
}
