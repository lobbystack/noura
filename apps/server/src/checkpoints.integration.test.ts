import { createApp } from './app';
import { digest, signingBytes, operation } from './protocol';
import { describe, expect, test } from 'bun:test';
import { randomBytes, sign } from 'node:crypto';
import { fixture } from './protocol.test';
import {
	accessDigest,
	accessSigningBytes,
	keySigningBytes,
	setAccess,
	type AccessPolicy,
} from './access';
import {
	accessTransition,
	checkpointSigningBytes,
	readTransition,
	stageTransition,
	transitionSigningBytes,
	type AccessTransition,
	type EncryptedCheckpoint,
} from './checkpoints';
import { SyncStore } from './store';
import {
	capabilitySigningBytes,
	capabilityDigest,
	putWorkspaceCapability,
	type WorkspaceCapability,
} from './capabilities';
import {
	activationSigningBytes,
	commitObjectActivation,
	readObjectActivation,
	stageObjectActivation,
} from './activations';
import type { ObjectActivation } from '../../../packages/shared/src/generated/ObjectActivation';

async function setup() {
	const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
	await store.migrate();
	const id = crypto.randomUUID();
	const owner = fixture(`owner_${id}`, `workspace_${id}`, 'object');
	const actor = {
		deviceId: `owner_${id}`,
		accountId: `account_${id}`,
		publicKey: owner.publicKey,
	};
	await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${actor.deviceId},${actor.accountId},${actor.publicKey})`;
	await store.createWorkspace(actor, `workspace_${id}`);
	const capability: WorkspaceCapability = {
		version: 1,
		workspaceId: `workspace_${id}`,
		collaborationVersion: 1,
		minimumClientVersion: 1,
		minimumRelayVersion: 1,
		deviceId: actor.deviceId,
		signature: '',
	};
	capability.signature = sign(
		null,
		capabilitySigningBytes(capability),
		owner.keys.privateKey,
	).toString('base64');
	await putWorkspaceCapability(store, actor, capability);
	await store.createObject(actor, `workspace_${id}`, 'object');
	function transition(
		coveredSequence = '0',
		epoch = 2,
		revision = '1',
		previousPolicyDigest: string | null = null,
		blobs: AccessTransition['blobs'] = [],
	): AccessTransition {
		const envelope = {
			deviceId: actor.deviceId,
			wrappedKey: randomBytes(80).toString('base64'),
		};
		const policy: AccessPolicy = {
			version: 1,
			workspaceId: `workspace_${id}`,
			revision,
			previousPolicyDigest,
			deviceId: actor.deviceId,
			members: [{ accountId: actor.accountId, role: 'owner' }],
			objects: [
				{
					objectId: 'object',
					epoch,
					grants: [],
					envelopes: [
						{
							...envelope,
							signature: sign(
								null,
								keySigningBytes(
									`workspace_${id}`,
									'object',
									epoch,
									actor.deviceId,
									envelope,
								),
								owner.keys.privateKey,
							).toString('base64'),
						},
					],
				},
			],
			signature: '',
		};
		policy.signature = sign(
			null,
			accessSigningBytes(policy),
			owner.keys.privateKey,
		).toString('base64');
		const checkpoint: EncryptedCheckpoint = {
			version: 1,
			generation: crypto.randomUUID(),
			coveredSequence,
			payload: owner.make(undefined, epoch, revision),
			signature: '',
		};
		checkpoint.signature = sign(
			null,
			checkpointSigningBytes(checkpoint),
			owner.keys.privateKey,
		).toString('base64');
		const result: AccessTransition = {
			version: blobs.length ? 2 : 1,
			transitionId: crypto.randomUUID(),
			coveredSequence,
			policy,
			checkpoints: [checkpoint],
			...(blobs.length ? { blobs } : {}),
			signature: '',
		};
		result.signature = sign(
			null,
			transitionSigningBytes(result),
			owner.keys.privateKey,
		).toString('base64');
		return accessTransition(result);
	}
	return { store, actor, owner, transition, capability };
}

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'checkpoint transition transactions',
	() => {
		test('text generations reject stale updates and whole-file downgrade writes', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const value = transition();
				value.policy.version = 2;
				value.policy.objects[0]!.document = {
					generation: value.checkpoints[0]!.generation,
					mode: 'text',
				};
				value.policy.signature = sign(
					null,
					accessSigningBytes(value.policy),
					owner.keys.privateKey,
				).toString('base64');
				value.signature = sign(
					null,
					transitionSigningBytes(value),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, accessTransition(value));
				await setAccess(store, actor, value.policy, value);
				const workspace = value.policy.workspaceId;
				const update = {
					...owner.make(undefined, 2, '1'),
					version: 2 as const,
					generation: value.checkpoints[0]!.generation,
					kind: 'text' as const,
				};
				update.signature = sign(
					null,
					signingBytes(update),
					owner.keys.privateKey,
				).toString('base64');
				await store.push(actor, workspace, [operation(update)]);
				await expect(
					store.push(actor, workspace, [owner.make(undefined, 2, '1')]),
				).rejects.toThrow('sync.generation_changed');
				for (const changed of [
					{ generation: 'stale' },
					{ kind: 'file' as const },
				]) {
					const rejected = {
						...update,
						...changed,
						operationId: crypto.randomUUID(),
					};
					rejected.signature = sign(
						null,
						signingBytes(rejected),
						owner.keys.privateKey,
					).toString('base64');
					await expect(
						store.push(actor, workspace, [operation(rejected)]),
					).rejects.toThrow();
				}
				const pulled = await store.pull(actor, workspace, '0');
				expect(pulled.operations).toHaveLength(1);
				expect(pulled.operations[0]!.generation).toBe(update.generation);
				expect(pulled.operations[0]!.kind).toBe('text');
			} finally {
				await store.close();
			}
		});
		test('staging is invisible, commit is atomic, retries reserve quota once and old history remains', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const first = owner.make();
				const workspace = first.workspaceId;
				await store.push(actor, workspace, [first]);
				const value = transition('1');
				await stageTransition(store, actor, value);
				const [before] =
					await store.db`SELECT used_bytes,access_revision FROM noura_workspaces WHERE id=${workspace}`;
				await stageTransition(store, actor, value);
				expect(await store.createObject(actor, workspace, 'object')).toBe(1);
				expect(
					(await readTransition(store, actor, workspace, value.transitionId))
						.committed,
				).toBe(false);
				await setAccess(store, actor, value.policy, value);
				await setAccess(store, actor, value.policy, value);
				const [after] =
					await store.db`SELECT used_bytes,access_revision FROM noura_workspaces WHERE id=${workspace}`;
				expect(after!.used_bytes).toBe(before!.used_bytes);
				expect(String(after!.access_revision)).toBe('1');
				expect(await store.createObject(actor, workspace, 'object')).toBe(2);
				expect(
					(await readTransition(store, actor, workspace, value.transitionId))
						.committed,
				).toBe(true);
				await expect(
					store.push(actor, workspace, [owner.make()]),
				).rejects.toThrow('sync.stale_epoch');
				expect(
					(await store.pull(actor, workspace, '0')).operations,
				).toHaveLength(1);
				const next = transition('1', 3, '2', accessDigest(value.policy));
				await expect(setAccess(store, actor, next.policy)).rejects.toThrow(
					'sync.checkpoint_required',
				);
			} finally {
				await store.close();
			}
		});
		test('experimental HTTP routes are opt-in and return the atomically committed checkpoint', async () => {
			const { store, actor, transition } = await setup();
			try {
				const value = transition();
				const token = randomBytes(32).toString('base64url');
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${actor.deviceId},now()+interval '1 hour')`;
				const headers = {
					Authorization: `Bearer ${token}`,
					'Content-Type': 'application/json',
				};
				const base = `/v1/workspaces/${value.policy.workspaceId}`;
				const disabled = createApp(store, { origin: 'http://localhost:1900' });
				const disabledCapabilities = await (
					await disabled.request('/v1/capabilities', { headers })
				).json();
				expect(disabledCapabilities.liveText).toEqual([]);
				expect(disabledCapabilities.objectActivations).toEqual([]);
				expect(
					(
						await disabled.request(`${base}/collaboration-capability`, {
							headers,
						})
					).status,
				).toBe(404);
				expect(
					(
						await disabled.request(`${base}/transitions`, {
							method: 'POST',
							headers,
							body: JSON.stringify(value),
						})
					).status,
				).toBe(404);
				const app = createApp(store, {
					origin: 'http://localhost:1900',
					checkpointTransitions: true,
				});
				expect(
					(await (await app.request('/v1/capabilities', { headers })).json())
						.liveText,
				).toEqual([1]);
				expect(
					(await app.request(`${base}/collaboration-capability`, { headers }))
						.status,
				).toBe(200);
				expect(
					(
						await app.request(`${base}/transitions`, {
							method: 'POST',
							headers,
							body: JSON.stringify(value),
						})
					).status,
				).toBe(200);
				expect(
					(await app.request(`${base}/objects/object/checkpoint`, { headers }))
						.status,
				).toBe(404);
				expect(
					(
						await app.request(
							`${base}/transitions/${value.transitionId}/commit`,
							{ method: 'POST', headers, body: '{}' },
						)
					).status,
				).toBe(200);
				expect(
					await (
						await app.request(`${base}/objects/object/checkpoint`, { headers })
					).json(),
				).toEqual(value.checkpoints[0]);

				const paused = createApp(store, {
					origin: 'http://localhost:1900',
					checkpointTransitions: true,
					collaborationRollout: false,
				});
				const pausedCapabilities = await (
					await paused.request('/v1/capabilities', { headers })
				).json();
				expect(pausedCapabilities.liveText).toEqual([1]);
				expect(pausedCapabilities.objectActivations).toEqual([1]);
				expect(
					await (
						await paused.request(`${base}/objects/object/checkpoint`, {
							headers,
						})
					).json(),
				).toEqual(value.checkpoints[0]);
				const blocked = await paused.request(
					`${base}/collaboration-capability`,
					{ method: 'PUT', headers, body: '{}' },
				);
				expect(blocked.status).toBe(409);
				expect((await blocked.json()).error.code).toBe(
					'sync.collaboration_rollout_disabled',
				);
			} finally {
				await store.close();
			}
		});

		test('a transition cannot commit until every bound next-epoch blob is durable', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const value = transition('0', 2, '1', null, [
					{
						objectId: 'object',
						epoch: 2,
						ciphertextDigest: 'a'.repeat(64),
						ciphertextSize: 1024,
					},
				]);
				value.signature = sign(
					null,
					transitionSigningBytes(value),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, accessTransition(value));
				await expect(
					setAccess(store, actor, value.policy, value),
				).rejects.toThrow('sync.transition_blob_incomplete');
				const status = await readTransition(
					store,
					actor,
					value.policy.workspaceId,
					value.transitionId,
				);
				expect(status.committed).toBe(false);
			} finally {
				await store.close();
			}
		});

		test('new readers cannot receive the current epoch after checkpoint enrollment', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const first = transition();
				await stageTransition(store, actor, first);
				await setAccess(store, actor, first.policy, first);
				const next = transition('0', 2, '2', accessDigest(first.policy));
				const reader = `reader_${crypto.randomUUID()}`;
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${reader},${reader},${fixture().publicKey})`;
				next.policy.objects[0]!.grants.push({
					accountId: reader,
					role: 'viewer',
				});
				next.policy.signature = sign(
					null,
					accessSigningBytes(next.policy),
					owner.keys.privateKey,
				).toString('base64');
				next.signature = sign(
					null,
					transitionSigningBytes(next),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, next);
				await expect(
					setAccess(store, actor, next.policy, next),
				).rejects.toThrow('sync.key_rotation_required');
				await expect(setAccess(store, actor, next.policy)).rejects.toThrow(
					'sync.access_transition_required',
				);
			} finally {
				await store.close();
			}
		});

		test('a newly checkpointed member receives no pre-invitation operation history', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const workspace = `workspace_${actor.deviceId.slice('owner_'.length)}`;
				await store.push(actor, workspace, [owner.make()]);
				const first = transition('1');
				await stageTransition(store, actor, first);
				await setAccess(store, actor, first.policy, first);

				const readerId = `reader_${crypto.randomUUID()}`;
				const reader = fixture(readerId, workspace, 'object');
				const readerActor = {
					deviceId: readerId,
					accountId: readerId,
					publicKey: reader.publicKey,
				};
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${readerId},${readerId},${reader.publicKey})`;
				const next = transition('1', 3, '2', accessDigest(first.policy));
				next.policy.members.push({ accountId: readerId, role: 'viewer' });
				next.policy.members.sort((a, b) =>
					a.accountId.localeCompare(b.accountId),
				);
				const wrappedKey = randomBytes(80).toString('base64');
				next.policy.objects[0]!.envelopes.push({
					deviceId: readerId,
					wrappedKey,
					signature: sign(
						null,
						keySigningBytes(workspace, 'object', 3, actor.deviceId, {
							deviceId: readerId,
							wrappedKey,
						}),
						owner.keys.privateKey,
					).toString('base64'),
				});
				next.policy.objects[0]!.envelopes.sort((a, b) =>
					a.deviceId.localeCompare(b.deviceId),
				);
				next.policy.signature = sign(
					null,
					accessSigningBytes(next.policy),
					owner.keys.privateKey,
				).toString('base64');
				next.signature = sign(
					null,
					transitionSigningBytes(next),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, accessTransition(next));
				await setAccess(store, actor, next.policy, next);

				const initial = await store.pull(readerActor, workspace, '0');
				expect(initial.operations).toHaveLength(0);
				expect(initial.cursor).toBe('1');
				await store.push(actor, workspace, [owner.make(undefined, 3, '2')]);
				const future = await store.pull(readerActor, workspace, '0');
				expect(
					future.operations.map((operation) => operation.sequence),
				).toEqual(['2']);
			} finally {
				await store.close();
			}
		});

		test('a write between stage and commit invalidates the boundary without changing access', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const value = transition();
				await stageTransition(store, actor, value);
				await store.push(actor, value.policy.workspaceId, [owner.make()]);
				await expect(
					setAccess(store, actor, value.policy, value),
				).rejects.toThrow('sync.transition_stale');
				expect(
					await store.createObject(actor, value.policy.workspaceId, 'object'),
				).toBe(1);
				expect(
					(
						await readTransition(
							store,
							actor,
							value.policy.workspaceId,
							value.transitionId,
						)
					).committed,
				).toBe(false);
			} finally {
				await store.close();
			}
		});
		test('object activation is invisible until an idempotent atomic commit', async () => {
			const { store, actor, owner, transition, capability } = await setup();
			try {
				const access = transition();
				access.policy.version = 2;
				access.policy.objects[0]!.document = {
					generation: access.checkpoints[0]!.generation,
					mode: 'text',
				};
				access.policy.signature = sign(
					null,
					accessSigningBytes(access.policy),
					owner.keys.privateKey,
				).toString('base64');
				access.signature = sign(
					null,
					transitionSigningBytes(access),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, accessTransition(access));
				await setAccess(store, actor, access.policy, access);

				const objectId = `created_${crypto.randomUUID()}`;
				const generation = crypto.randomUUID();
				const payload = owner.make(undefined, 1, '1');
				payload.objectId = objectId;
				payload.signature = sign(
					null,
					signingBytes(payload),
					owner.keys.privateKey,
				).toString('base64');
				const checkpoint: EncryptedCheckpoint = {
					version: 1,
					generation,
					coveredSequence: '0',
					payload,
					signature: '',
				};
				checkpoint.signature = sign(
					null,
					checkpointSigningBytes(checkpoint),
					owner.keys.privateKey,
				).toString('base64');
				const unsignedEnvelope = {
					deviceId: actor.deviceId,
					wrappedKey: randomBytes(80).toString('base64'),
				};
				const envelope = {
					...unsignedEnvelope,
					signature: sign(
						null,
						keySigningBytes(
							access.policy.workspaceId,
							objectId,
							1,
							actor.deviceId,
							unsignedEnvelope,
						),
						owner.keys.privateKey,
					).toString('base64'),
				};
				const activation: ObjectActivation = {
					version: 1,
					activationId: crypto.randomUUID(),
					workspaceId: access.policy.workspaceId,
					policyRevision: '1',
					coveredSequence: '0',
					capabilityDigest: capabilityDigest(capability),
					deviceId: actor.deviceId,
					document: { generation, mode: 'text' },
					envelopes: [envelope],
					checkpoint,
					signature: '',
				};
				activation.signature = sign(
					null,
					activationSigningBytes(activation),
					owner.keys.privateKey,
				).toString('base64');

				const staged = await stageObjectActivation(store, actor, activation);
				expect(staged.committed).toBe(false);
				expect(await stageObjectActivation(store, actor, activation)).toEqual(
					staged,
				);
				await expect(
					store.createObject(actor, activation.workspaceId, objectId),
				).rejects.toThrow('sync.object_activation_required');
				await commitObjectActivation(
					store,
					actor,
					activation.workspaceId,
					activation.activationId,
				);
				expect(
					(
						await readObjectActivation(
							store,
							actor,
							activation.workspaceId,
							activation.activationId,
						)
					).committed,
				).toBe(true);
				expect(
					await store.createObject(actor, activation.workspaceId, objectId),
				).toBe(1);
				await commitObjectActivation(
					store,
					actor,
					activation.workspaceId,
					activation.activationId,
				);
				const [stored] =
					await store.db`SELECT checkpoint FROM noura_activation_checkpoints WHERE workspace_id=${activation.workspaceId} AND object_id=${objectId}`;
				expect(stored!.checkpoint).toEqual(checkpoint);
			} finally {
				await store.close();
			}
		});
		test('reused transaction identities and quota exhaustion leave staged state intact', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const value = transition();
				await stageTransition(store, actor, value);
				const changed = transition();
				changed.transitionId = value.transitionId;
				changed.signature = sign(
					null,
					transitionSigningBytes(changed),
					owner.keys.privateKey,
				).toString('base64');
				await expect(stageTransition(store, actor, changed)).rejects.toThrow(
					'sync.transition_id_reused',
				);
				await store.db`UPDATE noura_workspaces SET quota_bytes=used_bytes WHERE id=${value.policy.workspaceId}`;
				await expect(
					stageTransition(store, actor, transition()),
				).rejects.toThrow('sync.quota_exceeded');
				await setAccess(store, actor, value.policy, value);
			} finally {
				await store.close();
			}
		});
		test('invalid recipient coverage rolls back checkpoint insertion and policy changes', async () => {
			const { store, actor, owner, transition } = await setup();
			try {
				const value = transition();
				value.policy.objects[0]!.envelopes = [];
				value.policy.signature = sign(
					null,
					accessSigningBytes(value.policy),
					owner.keys.privateKey,
				).toString('base64');
				value.signature = sign(
					null,
					transitionSigningBytes(value),
					owner.keys.privateKey,
				).toString('base64');
				await stageTransition(store, actor, value);
				await expect(
					setAccess(store, actor, value.policy, value),
				).rejects.toThrow('sync.key_envelopes_incomplete');
				const checkpoints =
					await store.db`SELECT 1 FROM noura_checkpoints WHERE workspace_id=${value.policy.workspaceId}`;
				expect(checkpoints).toHaveLength(0);
				expect(
					await store.createObject(actor, value.policy.workspaceId, 'object'),
				).toBe(1);
			} finally {
				await store.close();
			}
		});
	},
);
