/**
 * Browser device custody and encrypted workspace sync controller.
 *
 * The controller joins `@noura/browser-sync` (custody, enrollment, key
 * delivery, operation transport, file-change codec) and
 * `@noura/browser-sync-engine` (local replica reconciliation). It touches
 * browser-only globals lazily, so hosts can import it during SSR or prerender
 * and create the controller once the page is mounted.
 *
 * Custody rules enforced here:
 *
 * - The passphrase-wrapped device bundle is stored in origin-private storage
 *   outside canonical workspace files. Only wrapped ciphertext is persisted;
 *   an unwrapped `DeviceIdentity` lives in tab memory only.
 * - When OPFS is unavailable the controller reports `unavailable` and refuses
 *   to enroll rather than silently downgrading custody.
 * - A failed enrollment never overwrites the existing wrapped bundle.
 *
 * Reconciliation rules enforced here:
 *
 * - `enableSync` bootstraps a browser-only first device (remote workspace and
 *   sync object, self-wrapped object key, signed version-1 access policy) and
 *   persists a durable binding outside canonical workspace files. A durable
 *   binding is reused instead of creating a second remote workspace.
 * - `syncNow` reconciles through the worker-backed local replica once a binding
 *   exists; without one it returns a typed `not_configured` result. Before
 *   reconcile it provisions a remote object and key for every managed object the
 *   binding does not yet cover, wraps an existing object key to any newly active
 *   browser device that lacks an envelope, and uploads one rebuilt signed access
 *   policy. Nothing is faked: the local replica is the workspace worker, reached
 *   through the raw canonical file operations, and no result is reported without
 *   an actual reconcile.
 * - The unwrapped object key lives only in tab memory. The binding stores a
 *   `noura.sync.key.web` envelope wrapped to this browser device, never a
 *   plaintext object key. A delivered key that differs from the stored one is
 *   re-wrapped to this device and persisted the same way.
 * - The remote speaks the same bearer-token transport as the native client, and
 *   only same-origin requests are permitted.
 */

import {
	accessDigest,
	accessState,
	ATTACHMENT_ROOT,
	attachmentNameFromPath,
	BrowserSyncError,
	BrowserSyncErrorCode,
	bytesEqual,
	createDeviceIdentity,
	createRemoteObject,
	createRemoteWorkspace,
	createSameOriginFetch,
	decodeBase64,
	decodeRecipient,
	deviceFingerprintForCard,
	encodeBase64,
	enrollBrowserDevice,
	exportRecoveryKit as buildRecoveryKit,
	hasNativeActiveDevice,
	importRecoveryKit as openRecoveryKit,
	isAttachmentPathFor,
	isIdentifier,
	isPolicyRevisionChanged,
	nativeRecoveryObjects,
	parseAccessPolicy,
	parseNativeRecoveryKit,
	putRemoteAccessPolicy,
	randomBytes,
	randomIdentifier,
	receiveKeys,
	recoverNativeKeysToBrowserBinding as rewrapNativeKeys,
	requestDeviceChallenge,
	resolveNativeRecoverySigner,
	sealIdentity,
	signAccessPolicy,
	toBoundKey,
	toPolicyEnvelope,
	unlockDeviceIdentity,
	unwrapBoundKey,
	wrapKey,
	type AccessPolicy,
	type AccessPolicyEnvelope,
	type AccessPolicyObject,
	type AccessState,
	type BrowserSyncBindingRecord,
	type BrowserSyncBindingStore,
	type BrowserSyncBoundKey,
	type BrowserSyncBoundObject,
	type DeviceIdentity,
	type FetchLike,
	type KeyStore,
	type ParsedNativeRecoveryKit,
	type RecoveryKitFile,
	type WebKeyEnvelope,
	type WrappedKeyBundle,
} from '@noura/browser-sync';
import {
	BrowserSyncEngineError,
	BrowserSyncEngineErrorCode,
	createWorkerWorkspaceStorage,
	type SyncConflictResolution,
	type SyncState,
	type SyncStateStore,
} from '@noura/browser-sync-engine';
import type {
	BrowserWorkspaceFiles,
	BrowserWorkspaceObjectCard,
} from '@noura/browser-workspace';
import { MAX_BROWSER_ATTACHMENT_BYTES } from './attachment-fetcher';
import {
	runBrowserSyncSendAttachment,
	type BrowserSyncSendAttachmentOutcome,
} from './attachments';
import {
	openBrowserSyncBindingStore,
	openBrowserSyncKeyStore,
	openBrowserSyncStateStore,
} from './opfs';
import {
	runBrowserSyncReconcile,
	runBrowserSyncResolveConflict,
} from './reconcile';
import { failure, messageOf, ok } from './result';
import type {
	BrowserSyncConflictDetail,
	BrowserSyncDeviceCard,
	BrowserSyncDeviceInfo,
	BrowserSyncFailureCode,
	BrowserSyncObjectBinding,
	BrowserSyncResult,
	BrowserSyncStatus,
	BrowserSyncWorkspaceBinding,
	BrowserSyncWorkspaceSummary,
	SyncNowOutcome,
} from './types';
import { createBrowserSyncWorkspaceBinding } from './workspace-binding';

/** Default stable key for this browser's wrapped device bundle. */
export const DEFAULT_BROWSER_SYNC_BUNDLE_ID = 'browser-device';

/**
 * Compare two identifiers by Unicode code unit.
 *
 * Identifiers use the ASCII base64url alphabet, so code-unit order equals the
 * server's `COLLATE "C"` byte order. `localeCompare` is deliberately avoided:
 * it is locale-sensitive and would not match the server's ordering.
 */
function compareIdentifiers(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

/** Options accepted by {@link createBrowserSyncController}. */
export interface BrowserSyncControllerOptions {
	/** Account origin. Defaults to the current same origin. */
	origin?: string;
	/** Stable key for the wrapped bundle. Defaults to {@link DEFAULT_BROWSER_SYNC_BUNDLE_ID}. */
	bundleId?: string;
	/**
	 * Wrapped-bundle store. Omit to open the OPFS store; pass `null` to force the
	 * explicit `unavailable` state (used by tests and unsupported builds).
	 */
	keyStore?: KeyStore | null;
	/** Injected transport. Defaults to a same-origin `fetch` wrapper. */
	fetch?: FetchLike;
	/** Optional workspace reconcile binding. */
	binding?: BrowserSyncWorkspaceBinding | null;
	/** Raw worker file operations for the open browser workspace. */
	workspaceFiles?: BrowserWorkspaceFiles | null;
	/**
	 * Injected binding store. Omit to open the OPFS store; pass `null` to force
	 * an explicit unsupported result (used by tests).
	 */
	bindingStore?: BrowserSyncBindingStore | null;
	/**
	 * Injected engine state store. Omit to open the OPFS store; pass `null` to
	 * force an explicit unsupported result (used by tests).
	 */
	stateStore?: SyncStateStore | null;
}

/** Options for {@link BrowserSyncController.importNativeRecoveryKit}. */
export interface ImportNativeRecoveryKitOptions {
	/** User-supplied native `AGE-SECRET-KEY-...` recovery identity secret. */
	recoveryIdentity: string;
	/**
	 * Caller-pinned base64 Ed25519 recovery signer public key. When the kit also
	 * records one, the two must match; neither is trusted alone.
	 */
	recoverySignerPublic?: string;
	/** Local browser workspace the recovered binding attaches to. */
	localWorkspaceId?: string;
}

/**
 * Browser device custody and sync controller.
 *
 * The UI creates one of these on mount. Custody actions are async because the
 * passphrase KDF is intentionally slow; `status()` and `device()` are cheap and
 * synchronous for rendering.
 */
export class BrowserSyncController {
	readonly #bundleId: string;
	readonly #origin: string;
	readonly #keyStore: KeyStore | null;
	readonly #fetch: FetchLike;
	#binding: BrowserSyncWorkspaceBinding | null;
	#bundle: WrappedKeyBundle | null = null;
	#identity: DeviceIdentity | null = null;
	#error: string | null = null;
	#workspaceFiles: BrowserWorkspaceFiles | null;
	#bindingStore: BrowserSyncBindingStore | null | undefined;
	#openedBindingStore: { id: string; store: BrowserSyncBindingStore } | null =
		null;
	#stateStore: SyncStateStore | null | undefined;
	#localWorkspaceId: string | null = null;
	#record: BrowserSyncBindingRecord | null = null;

	constructor(options: {
		bundleId: string;
		origin: string;
		keyStore: KeyStore | null;
		fetch: FetchLike;
		binding: BrowserSyncWorkspaceBinding | null;
		workspaceFiles: BrowserWorkspaceFiles | null;
		bindingStore: BrowserSyncBindingStore | null | undefined;
		stateStore: SyncStateStore | null | undefined;
	}) {
		this.#bundleId = options.bundleId;
		this.#origin = options.origin;
		this.#keyStore = options.keyStore;
		this.#fetch = options.fetch;
		this.#binding = options.binding;
		this.#workspaceFiles = options.workspaceFiles;
		this.#bindingStore = options.bindingStore;
		this.#stateStore = options.stateStore;
	}

	/** Load persisted custody metadata. Never unlocks key material. */
	async init(): Promise<void> {
		if (!this.#keyStore) return;
		try {
			this.#bundle = (await this.#keyStore.read(this.#bundleId)) ?? null;
		} catch (error) {
			this.#error = messageOf(error);
		}
	}

	/** Current custody/availability state. */
	status(): BrowserSyncStatus {
		if (!this.#keyStore) return 'unavailable';
		if (this.#identity) return this.#identity.token ? 'enrolled' : 'unlocked';
		if (this.#error) return 'error';
		return 'locked';
	}

	/** Non-secret device projection, or `null` before any custody exists. */
	device(): BrowserSyncDeviceInfo | null {
		const deviceId = this.#identity?.deviceId ?? this.#bundle?.deviceId;
		if (!deviceId) return null;
		return { deviceId, enrolled: Boolean(this.#identity?.token) };
	}

	/** Latest failure message, if the controller is in the `error` state. */
	error(): string | null {
		return this.#error;
	}

	/** Replace the workspace reconcile binding (or clear it). */
	setBinding(binding: BrowserSyncWorkspaceBinding | null): void {
		this.#binding = binding;
	}

	/** Provide the raw worker file operations for the open browser workspace. */
	setWorkspaceFiles(files: BrowserWorkspaceFiles | null): void {
		this.#workspaceFiles = files;
	}

	/**
	 * Enable encrypted sync for the open browser workspace.
	 *
	 * When a durable binding already exists for `workspaceId`, it is reused and
	 * no remote workspace is created. Otherwise this bootstraps a browser-only
	 * first device exactly as the reference flow does: create the remote
	 * workspace and sync object, generate an object key and wrap it to this
	 * device, sign and upload a version-1 access policy, and persist the binding.
	 *
	 * Only public identity material, the access-policy revision, and the
	 * self-wrapped object key are persisted; the unwrapped object key stays in
	 * tab memory. No canonical workspace file is used for adapter state.
	 */
	async enableSync(input: {
		workspaceId: string;
		workspaceFiles?: BrowserWorkspaceFiles | null;
		create?: boolean;
	}): Promise<BrowserSyncResult<BrowserSyncWorkspaceSummary>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		const identity = this.#identity;
		if (!identity) {
			return failure('locked', 'Unlock this browser to turn on sync.');
		}
		if (!identity.token) {
			return failure('not_configured', 'Set up this browser for sync first.');
		}
		const workspaceFiles = input.workspaceFiles ?? this.#workspaceFiles;
		if (!workspaceFiles) {
			return failure('not_configured', 'Open a workspace to sync it.');
		}
		if (!this.#origin) {
			return failure('not_configured', "Sync isn't available on this page.");
		}
		try {
			const bindingStore = await this.#resolveBindingStore(input.workspaceId);
			if (!bindingStore) {
				return failure(
					'unavailable',
					"This browser can't save sync settings. Try another browser.",
				);
			}
			this.#localWorkspaceId = input.workspaceId;
			const existing = await this.#loadRecord(input.workspaceId);
			if (existing) {
				const binding = await this.#buildBinding(
					existing,
					identity,
					workspaceFiles,
				);
				if (!binding) {
					return failure(
						'unavailable',
						"This browser can't open the sync data for this workspace.",
					);
				}
				this.#binding = binding;
				this.#record = existing;
				this.#workspaceFiles = workspaceFiles;
				return ok(await this.workspaceSummary());
			}
			if (input.create === false) {
				return failure(
					'not_configured',
					"Sync isn't on for this workspace yet.",
				);
			}
			const { binding, record } = await this.#bootstrapBinding(
				input.workspaceId,
				identity,
				workspaceFiles,
				bindingStore,
			);
			this.#binding = binding;
			this.#record = record;
			this.#workspaceFiles = workspaceFiles;
			return ok(await this.workspaceSummary());
		} catch (error) {
			this.#error = messageOf(error);
			return failure('sync_failed', this.#error);
		}
	}

	/** Reuse a durable binding without creating a remote workspace. */
	async resumeSync(input: {
		workspaceId: string;
		workspaceFiles?: BrowserWorkspaceFiles | null;
	}): Promise<BrowserSyncResult<BrowserSyncWorkspaceSummary>> {
		return this.enableSync({ ...input, create: false });
	}

	/**
	 * Export this device's wrapped bundle and durable binding as an encrypted,
	 * user-held recovery kit.
	 *
	 * Requires unlocked custody and a loaded binding. The returned kit is
	 * AES-256-GCM ciphertext under a passphrase-derived key; no plaintext key
	 * material is ever returned. A kit restores this same device identity on
	 * another browser, not a new device.
	 */
	async exportRecoveryKit(
		passphrase: string,
	): Promise<BrowserSyncResult<RecoveryKitFile>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		if (!passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter a passphrase for the recovery kit.',
			);
		}
		if (!this.#identity || !this.#bundle) {
			return failure('locked', 'Unlock this browser to save a recovery kit.');
		}
		try {
			const record = this.#record ?? (await this.#loadRecord());
			if (!record) {
				return failure(
					'not_configured',
					'Turn on sync for this workspace before you save a recovery kit.',
				);
			}
			const kit = await buildRecoveryKit({
				bundle: this.#bundle,
				binding: record,
				passphrase,
			});
			return ok(kit);
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
	}

	/**
	 * Restore a device's wrapped bundle and binding from an encrypted recovery
	 * kit on another browser.
	 *
	 * The recovered bundle is sealed with the original device passphrase, which
	 * this browser does not hold, so it is stored as-is under its own bundle id
	 * and the controller stays locked until the user unlocks it with that
	 * passphrase. The binding is re-keyed to `localWorkspaceId` and persisted, so
	 * a later unlock builds the same usable binding. Requires an available key
	 * store; a wrong passphrase or tampered kit is rejected before any write.
	 */
	async importRecoveryKit(
		file: unknown,
		passphrase: string,
		localWorkspaceId: string,
	): Promise<BrowserSyncResult<BrowserSyncWorkspaceSummary>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		if (!passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter the recovery kit passphrase.',
			);
		}
		if (!isIdentifier(localWorkspaceId)) {
			return failure('not_configured', 'This workspace has an invalid id.');
		}
		let recovered: {
			bundle: WrappedKeyBundle;
			binding: BrowserSyncBindingRecord;
		};
		try {
			recovered = await openRecoveryKit(file, passphrase);
		} catch (error) {
			this.#error = messageOf(error);
			return failure(
				error instanceof BrowserSyncError &&
					error.code === BrowserSyncErrorCode.PassphraseRejected
					? 'passphrase_rejected'
					: 'custody_failed',
				this.#error,
			);
		}
		const { bundle, binding } = recovered;
		if (!isIdentifier(binding.workspaceId)) {
			return failure('not_configured', 'This recovery kit is damaged.');
		}
		const store = await this.#resolveBindingStore(localWorkspaceId);
		if (!store) {
			return failure(
				'unavailable',
				"This browser can't save sync settings. Try another browser.",
			);
		}
		try {
			await this.#keyStore.write(bundle.id, bundle);
			const next: BrowserSyncBindingRecord = {
				...binding,
				localWorkspaceId,
			};
			await store.write(next);
			this.#bundle = bundle;
			this.#identity = null;
			this.#localWorkspaceId = localWorkspaceId;
			this.#record = next;
			this.#binding = null;
			this.#error = null;
			return ok(await this.workspaceSummary({ workspaceId: localWorkspaceId }));
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
	}

	/**
	 * Import a native `noura.sync.recovery` recovery kit and re-wrap its object
	 * keys to this browser device.
	 *
	 * The signed public recovery object is verified against a recovery signer
	 * that the caller pins or the kit self-describes consistently; if the two
	 * disagree the import is refused rather than trusting either. Each recovered
	 * object key is unwrapped with the supplied recovery identity, immediately
	 * re-wrapped as a `noura.sync.key.web` envelope addressed to this unlocked
	 * browser device, and persisted as a durable binding. The recovery identity
	 * and the plaintext keys stay in memory; only ciphertext and public pins are
	 * written. When no unlocked device bundle exists this returns `locked` and
	 * creates nothing.
	 */
	async importNativeRecoveryKit(
		file: unknown,
		options: ImportNativeRecoveryKitOptions,
	): Promise<BrowserSyncResult<BrowserSyncWorkspaceSummary>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		const identity = this.#identity;
		if (!this.#bundle || !identity) {
			return failure(
				'locked',
				'Unlock this browser to use a desktop recovery kit.',
			);
		}
		if (!identity.token) {
			return failure('not_configured', 'Set up this browser for sync first.');
		}
		const localWorkspaceId = options.localWorkspaceId ?? this.#localWorkspaceId;
		if (!localWorkspaceId || !isIdentifier(localWorkspaceId)) {
			return failure(
				'not_configured',
				'Open a workspace to use a desktop recovery kit.',
			);
		}
		let parsed: ParsedNativeRecoveryKit;
		try {
			parsed = parseNativeRecoveryKit(file);
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
		const recoveryIdentity = options.recoveryIdentity?.trim() ?? '';
		if (!recoveryIdentity) {
			return failure('custody_failed', 'Paste the recovery key from the kit.');
		}
		if (
			parsed.recoveryIdentity !== null &&
			parsed.recoveryIdentity !== recoveryIdentity
		) {
			return failure(
				'custody_failed',
				"That recovery key doesn't match this kit.",
			);
		}
		const signer = resolveNativeRecoverySigner(
			parsed,
			options.recoverySignerPublic?.trim(),
		);
		if (!signer.ok) return failure('custody_failed', signer.message);
		const recoveryObjects = nativeRecoveryObjects(parsed.recovery);
		if (!recoveryObjects) {
			return failure(
				'custody_failed',
				'This recovery kit has no keys to restore.',
			);
		}
		const store = await this.#resolveBindingStore(localWorkspaceId);
		if (!store) {
			return failure(
				'unavailable',
				"This browser can't save sync settings. Try another browser.",
			);
		}
		// A recovery kit is untrusted input. Never let it silently repoint an
		// already-configured workspace at a different remote workspace.
		let existing: BrowserSyncBindingRecord | null = null;
		try {
			existing = await store.read();
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
		if (
			existing &&
			existing.workspaceId !== parsed.recovery.config.workspaceId
		) {
			return failure(
				'custody_failed',
				"This workspace already syncs with a different workspace, so you can't restore this kit here.",
			);
		}
		try {
			const result = await rewrapNativeKeys({
				recovery: parsed.recovery,
				recoveryIdentity,
				trustedRecoverySigner: signer.value,
				device: identity,
				localWorkspaceId,
				revision: '1',
				objectId: recoveryObjects.objectId,
				objects: recoveryObjects.objects,
			});
			await store.write(result.binding);
			this.#localWorkspaceId = localWorkspaceId;
			this.#record = result.binding;
			this.#binding = null;
			this.#error = null;
			if (this.#workspaceFiles) {
				try {
					this.#binding = await this.#buildBinding(
						result.binding,
						identity,
						this.#workspaceFiles,
					);
				} catch {
					this.#binding = null;
				}
			}
			return ok(await this.workspaceSummary({ workspaceId: localWorkspaceId }));
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
	}

	async #bootstrapBinding(
		localWorkspaceId: string,
		identity: DeviceIdentity,
		workspaceFiles: BrowserWorkspaceFiles,
		bindingStore: BrowserSyncBindingStore,
	): Promise<{
		binding: BrowserSyncWorkspaceBinding;
		record: BrowserSyncBindingRecord;
	}> {
		const cards = await workspaceFiles.listObjects();
		if (cards.length === 0) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidOperation,
				'This workspace has no notes, tasks, or projects to sync yet.',
			);
		}
		const workspaceId = `ws_${randomIdentifier()}`;
		const policyRevision = '1';
		const challenge = await requestDeviceChallenge(this.#fetch);
		await createRemoteWorkspace({
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			workspaceId,
		});
		const objects: Record<string, BrowserSyncBoundObject> = {};
		const objectsById = new Map<string, BrowserSyncObjectBinding>();
		const objectKeys = new Map<string, Uint8Array>();
		const policyObjects: AccessPolicy['objects'] = [];
		// The access policy must carry an envelope for every active device, so wrap
		// each object key to each browser-capable device, not just this one.
		const activeDevices = await this.#activeWebDevices(workspaceId, identity);
		for (const card of cards) {
			const objectId = `obj_${randomIdentifier()}`;
			const epoch = await createRemoteObject({
				origin: this.#origin,
				token: identity.token,
				fetch: this.#fetch,
				workspaceId,
				objectId,
			});
			const objectKey = randomBytes(32);
			const envelopes: BrowserSyncBoundKey[] = [];
			for (const device of activeDevices) {
				const envelope = await wrapKey({
					workspace_id: workspaceId,
					object_id: objectId,
					epoch,
					signing_device: identity.deviceId,
					device_id: device.deviceId,
					signing_secret: encodeBase64(identity.signingSeed),
					recipient_public: encodeBase64(decodeRecipient(device.recipient)),
					object_key: encodeBase64(objectKey),
					ephemeral_secret: encodeBase64(randomBytes(32)),
					salt: encodeBase64(randomBytes(32)),
					nonce: encodeBase64(randomBytes(12)),
				});
				envelopes.push(toBoundKey(envelope, device.deviceId));
			}
			const boundKey = envelopes.find(
				(envelope) => envelope.deviceId === identity.deviceId,
			);
			if (!boundKey)
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidIdentity,
					"Your account doesn't list this browser as an active device yet.",
				);
			objects[objectId] = {
				path: card.path,
				localObjectId: card.id,
				epoch,
				policyRevision,
				key: boundKey,
			};
			objectsById.set(objectId, {
				objectId,
				path: card.path,
				localObjectId: card.id,
				epoch,
				policyRevision,
			});
			objectKeys.set(objectId, objectKey);
			policyObjects.push({
				objectId,
				epoch,
				grants: [],
				envelopes,
			});
		}
		const primaryId = cards[0] ? [...objectsById.keys()][0]! : '';
		const draft: Omit<AccessPolicy, 'signature'> = {
			version: 1,
			workspaceId,
			revision: policyRevision,
			previousPolicyDigest: null,
			deviceId: identity.deviceId,
			members: [{ accountId: challenge.accountId, role: 'owner' }],
			objects: policyObjects,
		};
		const policy = await signAccessPolicy(draft, identity);
		await putRemoteAccessPolicy({
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			workspaceId,
			policy,
		});
		const record: BrowserSyncBindingRecord = {
			version: 1,
			localWorkspaceId,
			workspaceId,
			revision: policy.revision,
			objectId: primaryId,
			objects,
			pinnedSigners: {
				[identity.deviceId]: encodeBase64(identity.signingPublic),
			},
		};
		await bindingStore.write(record);
		const state =
			this.#stateStore !== undefined
				? this.#stateStore
				: await openBrowserSyncStateStore(workspaceId);
		if (!state) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.RequestFailed,
				"This browser can't open the sync data for this workspace.",
			);
		}
		const primary = objects[primaryId];
		if (!primary) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidOperation,
				'This workspace has nothing to sync yet.',
			);
		}
		const binding = await createBrowserSyncWorkspaceBinding({
			workspaceId,
			objectId: primaryId,
			epoch: primary.epoch,
			policyRevision,
			objectKeys,
			objects: objectsById,
			pinnedSigners: new Map([[identity.deviceId, identity.signingPublic]]),
			workspace: createWorkerWorkspaceStorage(workspaceFiles),
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			state,
		});
		if (!binding) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.RequestFailed,
				"This browser can't open the sync data for this workspace.",
			);
		}
		return { binding, record };
	}

	async #buildBinding(
		record: BrowserSyncBindingRecord,
		identity: DeviceIdentity,
		workspaceFiles: BrowserWorkspaceFiles,
	): Promise<BrowserSyncWorkspaceBinding | null> {
		const objectKeys = new Map<string, Uint8Array>();
		const objects = new Map<string, BrowserSyncObjectBinding>();
		for (const [objectId, bound] of Object.entries(record.objects)) {
			objectKeys.set(
				objectId,
				await unwrapBoundKey(record, objectId, bound, identity),
			);
			objects.set(objectId, {
				objectId,
				path: bound.path,
				...(bound.localObjectId === undefined
					? {}
					: { localObjectId: bound.localObjectId }),
				...(bound.unmapped === true ? { unmapped: true } : {}),
				epoch: bound.epoch,
				policyRevision: bound.policyRevision,
			});
		}
		const pinnedSigners = new Map<string, Uint8Array>();
		for (const [deviceId, value] of Object.entries(record.pinnedSigners)) {
			pinnedSigners.set(deviceId, decodeBase64(value));
		}
		const primary = record.objects[record.objectId];
		if (!primary) return null;
		const state =
			this.#stateStore !== undefined
				? this.#stateStore
				: await openBrowserSyncStateStore(record.workspaceId);
		if (!state) return null;
		return createBrowserSyncWorkspaceBinding({
			workspaceId: record.workspaceId,
			objectId: record.objectId,
			epoch: primary.epoch,
			policyRevision: primary.policyRevision,
			objectKeys,
			objects,
			pinnedSigners,
			workspace: createWorkerWorkspaceStorage(workspaceFiles),
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			state,
		});
	}

	/**
	 * Generate, enroll, and persist a new browser device identity.
	 *
	 * The wrapped bundle is written only after enrollment succeeds, so a failed
	 * attempt leaves the previous custody untouched.
	 */
	async enroll(input: {
		passphrase: string;
	}): Promise<BrowserSyncResult<BrowserSyncStatus>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		if (!input.passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter a passphrase to protect this browser.',
			);
		}
		if (!this.#origin) {
			return failure('not_configured', "You can't set up sync on this page.");
		}
		try {
			const challenge = await requestDeviceChallenge(this.#fetch);
			const draft = await createDeviceIdentity({
				passphrase: input.passphrase,
				bundleId: this.#bundleId,
			});
			const identity = await unlockDeviceIdentity(draft, input.passphrase);
			const token = await enrollBrowserDevice({
				origin: this.#origin,
				accountId: challenge.accountId,
				challenge: challenge.challenge,
				identity,
				fetch: this.#fetch,
			});
			identity.token = token;
			const resealed = await sealIdentity(identity, input.passphrase, {
				bundleId: this.#bundleId,
			});
			await this.#keyStore.write(this.#bundleId, resealed);
			this.#bundle = resealed;
			this.#identity = identity;
			this.#error = null;
			return ok(this.status());
		} catch (error) {
			this.#error = messageOf(error);
			return failure(enrollFailureCode(error), this.#error);
		}
	}

	/** Unlock the persisted wrapped bundle with the passphrase. */
	async unlock(input: {
		passphrase: string;
	}): Promise<BrowserSyncResult<BrowserSyncStatus>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		if (!input.passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter your passphrase to unlock this browser.',
			);
		}
		let bundle = this.#bundle;
		try {
			bundle = (await this.#keyStore.read(this.#bundleId)) ?? bundle;
		} catch (error) {
			this.#error = messageOf(error);
			return failure('custody_failed', this.#error);
		}
		if (!bundle) {
			return failure('not_configured', 'Set up this browser for sync first.');
		}
		try {
			const identity = await unlockDeviceIdentity(bundle, input.passphrase);
			this.#bundle = bundle;
			this.#identity = identity;
			this.#error = null;
			return ok(this.status());
		} catch (error) {
			this.#identity = null;
			this.#error = messageOf(error);
			return failure(
				error instanceof BrowserSyncError &&
					error.code === BrowserSyncErrorCode.PassphraseRejected
					? 'passphrase_rejected'
					: 'custody_failed',
				this.#error,
			);
		}
	}

	/** Drop the in-memory identity; the wrapped bundle stays persisted. */
	lock(): BrowserSyncResult<BrowserSyncStatus> {
		this.#identity = null;
		this.#error = null;
		return ok(this.status());
	}

	/**
	 * Load the durable binding for the current local workspace without creating
	 * one. This lets a host re-establish the binding after a reload or before a
	 * component mounts, rather than reporting `not_configured` for a workspace
	 * that is already synchronized.
	 */
	async bindWorkspace(input: {
		workspaceId: string;
		workspaceFiles?: BrowserWorkspaceFiles | null;
	}): Promise<BrowserSyncResult<BrowserSyncWorkspaceSummary>> {
		if (!this.#keyStore) {
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		}
		const identity = this.#identity;
		if (!identity) {
			return failure('locked', 'Unlock this browser to sync this workspace.');
		}
		const workspaceFiles = input.workspaceFiles ?? this.#workspaceFiles;
		if (!workspaceFiles) {
			return failure('not_configured', 'Open a workspace to sync it.');
		}
		this.#localWorkspaceId = input.workspaceId;
		this.#workspaceFiles = workspaceFiles;
		try {
			const record = await this.#loadRecord(input.workspaceId);
			if (!record) {
				return failure(
					'not_configured',
					"Sync isn't on for this workspace yet.",
				);
			}
			const binding = await this.#buildBinding(
				record,
				identity,
				workspaceFiles,
			);
			if (!binding) {
				return failure(
					'unavailable',
					"This browser can't open the sync data for this workspace.",
				);
			}
			this.#binding = binding;
			return ok(await this.workspaceSummary());
		} catch (error) {
			this.#error = messageOf(error);
			return failure('sync_failed', this.#error);
		}
	}

	/**
	 * Resolve the durable binding store for one local workspace.
	 *
	 * An injected store is returned as-is. An OPFS store is opened per local
	 * workspace id and cached only for that id, so reusing the controller across
	 * workspaces never reads another workspace's record.
	 */
	async #resolveBindingStore(
		localId: string,
	): Promise<BrowserSyncBindingStore | null> {
		if (this.#bindingStore !== undefined) return this.#bindingStore;
		if (this.#openedBindingStore?.id === localId)
			return this.#openedBindingStore.store;
		const store = await openBrowserSyncBindingStore(localId);
		this.#openedBindingStore = store ? { id: localId, store } : null;
		return store;
	}

	/**
	 * Load the durable binding record if it is not already in memory. Only the
	 * wrapped record is read here; object keys are unwrapped later by
	 * {@link #buildBinding} with the unlocked identity.
	 */
	async #loadRecord(
		workspaceId?: string,
	): Promise<BrowserSyncBindingRecord | null> {
		const target = workspaceId ?? this.#localWorkspaceId;
		if (
			this.#record &&
			(target === undefined || this.#record.localWorkspaceId === target)
		)
			return this.#record;
		if (this.#record && target !== undefined) {
			// A different workspace was requested; drop the other workspace's state.
			this.#record = null;
			this.#binding = null;
		}
		const localId = target;
		if (!localId) return null;
		const store = await this.#resolveBindingStore(localId);
		if (!store) return null;
		let record: BrowserSyncBindingRecord | null;
		try {
			record = await store.read();
		} catch {
			return null;
		}
		if (!record) return null;
		this.#localWorkspaceId = localId;
		this.#record = record;
		return record;
	}

	/** Build the in-memory binding from the durable record and open files. */
	async #ensureBinding(
		workspaceId?: string,
	): Promise<BrowserSyncWorkspaceBinding | null> {
		if (this.#binding) return this.#binding;
		const identity = this.#identity;
		if (!identity) return null;
		const record = await this.#loadRecord(workspaceId);
		if (!record) return null;
		const workspaceFiles = this.#workspaceFiles;
		if (!workspaceFiles) return null;
		const binding = await this.#buildBinding(record, identity, workspaceFiles);
		if (!binding) return null;
		this.#binding = binding;
		return binding;
	}

	/**
	 * Refresh each bound object's path from the live managed-object list. A
	 * managed object that is new since bootstrap has no key and is left out;
	 * files whose path matches no object are unmanaged and never sealed.
	 */
	async #liveObjects(
		binding: BrowserSyncWorkspaceBinding,
	): Promise<ReadonlyMap<string, BrowserSyncObjectBinding>> {
		const base = binding.objects ?? new Map<string, BrowserSyncObjectBinding>();
		const files = this.#workspaceFiles;
		if (!files) return base;
		let cards: BrowserWorkspaceObjectCard[];
		try {
			cards = await files.listObjects();
		} catch {
			return base;
		}
		const byLocalId = new Map(cards.map((card) => [card.id, card]));
		const live = new Map(base);
		for (const [objectId, object] of live) {
			const card = object.localObjectId
				? byLocalId.get(object.localObjectId)
				: cards.find((candidate) => candidate.path === object.path);
			if (card) {
				live.set(objectId, {
					...object,
					localObjectId: card.id,
					livePath: card.path,
				});
			}
		}
		return live;
	}

	/** Wrap one object key to a specific device recipient. */
	async #wrapToDevice(input: {
		workspaceId: string;
		objectId: string;
		epoch: number;
		identity: DeviceIdentity;
		objectKey: Uint8Array;
		device: { deviceId: string; recipient: string };
	}): Promise<WebKeyEnvelope> {
		return wrapKey({
			workspace_id: input.workspaceId,
			object_id: input.objectId,
			epoch: input.epoch,
			signing_device: input.identity.deviceId,
			device_id: input.device.deviceId,
			signing_secret: encodeBase64(input.identity.signingSeed),
			recipient_public: encodeBase64(decodeRecipient(input.device.recipient)),
			object_key: encodeBase64(input.objectKey),
			ephemeral_secret: encodeBase64(randomBytes(32)),
			salt: encodeBase64(randomBytes(32)),
			nonce: encodeBase64(randomBytes(12)),
		});
	}

	/**
	 * Rebuild the signed access policy from the current one.
	 *
	 * Existing object entries, grants, documents, and envelopes are preserved.
	 * A key is wrapped to an active device that lacks an envelope for an existing
	 * object only when that key is available locally; otherwise the object is
	 * skipped without failing. Every new object is wrapped to every active device.
	 * Returns `null` when nothing changed, so the caller never uploads a no-op
	 * policy.
	 */
	async #buildProvisionedPolicy(input: {
		workspaceId: string;
		current: AccessPolicy;
		record: BrowserSyncBindingRecord;
		devices: Array<{ deviceId: string; recipient: string }>;
		planned: Array<{
			card: BrowserWorkspaceObjectCard;
			objectId: string;
			objectKey: Uint8Array;
			epoch: number;
		}>;
		identity: DeviceIdentity;
	}): Promise<{
		policy: AccessPolicy;
		recordObjects: Record<string, BrowserSyncBoundObject>;
	} | null> {
		const revision = (BigInt(input.current.revision) + 1n).toString();
		const recordObjects: Record<string, BrowserSyncBoundObject> = {};
		for (const [objectId, bound] of Object.entries(input.record.objects)) {
			recordObjects[objectId] = { ...bound, key: { ...bound.key } };
		}
		const objects: AccessPolicyObject[] = [];
		let changed = false;

		for (const currentObject of input.current.objects) {
			const envelopes = currentObject.envelopes.map((envelope) => ({
				...envelope,
			}));
			const present = new Set(envelopes.map((envelope) => envelope.deviceId));
			const bound = input.record.objects[currentObject.objectId];
			if (bound) {
				for (const device of input.devices) {
					if (present.has(device.deviceId)) continue;
					let objectKey: Uint8Array;
					try {
						objectKey = await unwrapBoundKey(
							input.record,
							currentObject.objectId,
							bound,
							input.identity,
						);
					} catch {
						// The key is not available locally; skip without failing.
						continue;
					}
					const envelope = await this.#wrapToDevice({
						workspaceId: input.workspaceId,
						objectId: currentObject.objectId,
						epoch: currentObject.epoch,
						identity: input.identity,
						objectKey,
						device,
					});
					envelopes.push(toPolicyEnvelope(envelope, device.deviceId));
					present.add(device.deviceId);
					changed = true;
				}
			}
			envelopes.sort((left, right) =>
				compareIdentifiers(left.deviceId, right.deviceId),
			);
			objects.push({ ...currentObject, envelopes });
		}

		for (const entry of input.planned) {
			const boundEnvelopes: BrowserSyncBoundKey[] = [];
			const envelopes: AccessPolicyEnvelope[] = [];
			for (const device of input.devices) {
				const envelope = await this.#wrapToDevice({
					workspaceId: input.workspaceId,
					objectId: entry.objectId,
					epoch: entry.epoch,
					identity: input.identity,
					objectKey: entry.objectKey,
					device,
				});
				envelopes.push(toPolicyEnvelope(envelope, device.deviceId));
				boundEnvelopes.push(toBoundKey(envelope, device.deviceId));
			}
			const self = boundEnvelopes.find(
				(envelope) => envelope.deviceId === input.identity.deviceId,
			);
			if (!self)
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidIdentity,
					"Your account doesn't list this browser as an active device yet.",
				);
			recordObjects[entry.objectId] = {
				path: entry.card.path,
				localObjectId: entry.card.id,
				epoch: entry.epoch,
				policyRevision: revision,
				key: self,
			};
			envelopes.sort((left, right) =>
				compareIdentifiers(left.deviceId, right.deviceId),
			);
			objects.push({
				objectId: entry.objectId,
				epoch: entry.epoch,
				grants: [],
				envelopes,
			});
			changed = true;
		}

		if (!changed) return null;

		objects.sort((left, right) =>
			compareIdentifiers(left.objectId, right.objectId),
		);
		const members = [...input.current.members]
			.sort((left, right) =>
				compareIdentifiers(left.accountId, right.accountId),
			)
			.map((member) => ({ ...member }));
		const draft: Omit<AccessPolicy, 'signature'> = {
			version: input.current.version,
			workspaceId: input.current.workspaceId,
			revision,
			previousPolicyDigest: await accessDigest(input.current),
			deviceId: input.identity.deviceId,
			members,
			objects,
		};
		const policy = await signAccessPolicy(draft, input.identity);
		return { policy, recordObjects };
	}

	/**
	 * Provision remote objects for managed objects created after bootstrap and
	 * wrap existing object keys to newly active browser devices.
	 *
	 * Runs from {@link syncNow}. A managed object with no bound remote object gets
	 * a new object, a fresh 32-byte key wrapped to every active browser device, and
	 * an entry in a rebuilt access policy. An active browser device missing an
	 * envelope for an existing bound object gets that key wrapped to it. A no-op
	 * never uploads a policy; a revision conflict re-reads access-state once and
	 * retries; any other failure surfaces. The updated binding and revision are
	 * persisted only after the policy upload succeeds.
	 */
	async #provisionObjects(
		binding: BrowserSyncWorkspaceBinding,
		identity: DeviceIdentity,
	): Promise<void> {
		const files = this.#workspaceFiles;
		if (!files || !this.#origin || !identity.token) return;
		const record = this.#record;
		if (!record || record.workspaceId !== binding.workspaceId) return;

		let cards: BrowserWorkspaceObjectCard[];
		try {
			cards = await files.listObjects();
		} catch {
			return;
		}
		const knownLocalIds = new Set<string>();
		const knownPaths = new Set<string>();
		for (const bound of Object.values(record.objects)) {
			if (bound.localObjectId) knownLocalIds.add(bound.localObjectId);
			knownPaths.add(bound.path);
		}
		const planned = cards
			.filter(
				(card) => !knownLocalIds.has(card.id) && !knownPaths.has(card.path),
			)
			.map((card) => ({
				card,
				objectId: `obj_${randomIdentifier()}`,
				objectKey: randomBytes(32),
				epoch: 0,
			}));

		for (let attempt = 0; attempt < 2; attempt += 1) {
			const state = await accessState({
				origin: this.#origin,
				token: identity.token,
				workspaceId: binding.workspaceId,
				fetch: this.#fetch,
			});
			const current = parseAccessPolicy(state.policy);
			if (!current) return;
			// A browser cannot wrap a new object key to a native `age` recipient. If a
			// native device is active, a browser-authored policy cannot cover a new
			// object, so leave it unprovisioned rather than upload an incomplete
			// policy. Existing objects keep syncing; the new files stay unmanaged.
			if (planned.length > 0 && hasNativeActiveDevice(state)) return;
			const devices = await this.#activeWebDevices(
				binding.workspaceId,
				identity,
				{ state, browserOnly: true },
			);
			if (attempt === 0) {
				for (const entry of planned) {
					entry.epoch = await createRemoteObject({
						origin: this.#origin,
						token: identity.token,
						fetch: this.#fetch,
						workspaceId: binding.workspaceId,
						objectId: entry.objectId,
					});
				}
			}
			const built = await this.#buildProvisionedPolicy({
				workspaceId: binding.workspaceId,
				current,
				record,
				devices,
				planned,
				identity,
			});
			if (!built) return;
			try {
				await putRemoteAccessPolicy({
					origin: this.#origin,
					token: identity.token,
					fetch: this.#fetch,
					workspaceId: binding.workspaceId,
					policy: built.policy,
				});
			} catch (error) {
				if (attempt === 0 && isPolicyRevisionChanged(error)) continue;
				throw error;
			}
			const store = await this.#resolveBindingStore(record.localWorkspaceId);
			if (!store) return;
			const next: BrowserSyncBindingRecord = {
				...record,
				revision: built.policy.revision,
				objects: built.recordObjects,
			};
			await store.write(next);
			this.#record = next;
			const rebuilt = await this.#buildBinding(next, identity, files);
			if (rebuilt) this.#binding = rebuilt;
			return;
		}
	}

	/**
	 * Pull keys wrapped to this device and merge them over the locally wrapped
	 * ones. A delivery failure is not fatal: the local self-wrapped keys stay in
	 * place, and the server is never a trust source, so nothing from a failed
	 * delivery is applied.
	 *
	 * A delivered key that differs from the stored self-wrapped key is re-wrapped
	 * to this device and persisted in the durable binding, so a later sync still
	 * has the key when the server is unreachable. Only the wrapped envelope is
	 * ever persisted; the plaintext key stays in tab memory.
	 */
	async #refreshObjectKeys(
		binding: BrowserSyncWorkspaceBinding,
		identity: DeviceIdentity,
	): Promise<Map<string, Uint8Array>> {
		const keys = new Map(binding.objectKeys);
		let delivered: Awaited<ReturnType<typeof receiveKeys>>;
		try {
			delivered = await receiveKeys({
				origin: this.#origin,
				token: identity.token,
				workspaceId: binding.workspaceId,
				deviceId: identity.deviceId,
				identity,
				pinnedSigners: binding.pinnedSigners,
				fetch: this.#fetch,
			});
		} catch {
			// Keep the locally wrapped keys; delivery is only a refresh.
			return keys;
		}
		for (const [objectId, key] of delivered.keys) keys.set(objectId, key);

		const record = this.#record;
		if (!record) return keys;
		const store = await this.#resolveBindingStore(record.localWorkspaceId);
		if (!store) return keys;

		const nextObjects = { ...record.objects };
		let changed = false;
		for (const [objectId, key] of delivered.keys) {
			const bound = record.objects[objectId];
			if (!bound) continue;
			let stored: Uint8Array | undefined;
			try {
				stored = await unwrapBoundKey(record, objectId, bound, identity);
			} catch {
				stored = undefined;
			}
			if (stored && bytesEqual(stored, key)) continue;
			const envelope = await this.#wrapToDevice({
				workspaceId: record.workspaceId,
				objectId,
				epoch: bound.epoch,
				identity,
				objectKey: key,
				device: { deviceId: identity.deviceId, recipient: identity.recipient },
			});
			nextObjects[objectId] = {
				...bound,
				key: toBoundKey(envelope, identity.deviceId),
			};
			changed = true;
		}
		if (changed) {
			const next: BrowserSyncBindingRecord = {
				...record,
				objects: nextObjects,
			};
			await store.write(next);
			this.#record = next;
		}
		return keys;
	}

	/** Project the durable state into a summary, or an unconfigured one. */
	#summaryFromState(
		workspaceId: string | null,
		state: SyncState | null,
	): BrowserSyncWorkspaceSummary {
		if (!state) {
			return {
				configured: false,
				workspaceId,
				cursor: '0',
				pending: 0,
				conflicts: 0,
				conflictDetails: [],
			};
		}
		return {
			configured: true,
			workspaceId,
			cursor: state.cursor,
			pending: state.outbox.length,
			conflicts: state.conflicts.length,
			conflictDetails: state.conflicts.map((conflict) => ({
				operationId: conflict.operationId,
				objectId: conflict.objectId,
				path: conflict.path,
				reason: conflict.reason,
			})),
		};
	}

	/**
	 * Durable cursor, pending outbox, and conflict detail for the bound workspace.
	 *
	 * When no binding is in memory this loads the durable record for
	 * `input.workspaceId`, or the last bound workspace, so a summary is available
	 * before the workspace component has mounted.
	 */
	async workspaceSummary(
		input: { workspaceId?: string } = {},
	): Promise<BrowserSyncWorkspaceSummary> {
		if (!this.#binding) {
			try {
				await this.#ensureBinding(input.workspaceId);
			} catch {
				// Fall through to the record-only summary below.
			}
		}
		const binding = this.#binding;
		if (binding) {
			try {
				return this.#summaryFromState(
					binding.workspaceId,
					await binding.state.read(),
				);
			} catch {
				return this.#summaryFromState(binding.workspaceId, null);
			}
		}
		const record = await this.#loadRecord(input.workspaceId).catch(() => null);
		if (!record) return this.#summaryFromState(null, null);
		const state =
			this.#stateStore !== undefined && this.#stateStore !== null
				? this.#stateStore
				: await openBrowserSyncStateStore(record.workspaceId);
		if (!state) return this.#summaryFromState(record.workspaceId, null);
		try {
			return this.#summaryFromState(record.workspaceId, await state.read());
		} catch {
			return this.#summaryFromState(record.workspaceId, null);
		}
	}

	/**
	 * Reconcile the bound workspace once, or return a typed non-success outcome.
	 *
	 * This never fabricates a result: without an unlocked enrolled identity, an
	 * explicit object-key map, and the injectable replica boundaries it reports
	 * `locked`, `not_configured`, or `unavailable` and performs no I/O. When no
	 * binding is in memory it loads the durable one for `input.workspaceId`.
	 */
	async syncNow(input: { workspaceId?: string } = {}): Promise<SyncNowOutcome> {
		if (!this.#keyStore) {
			return {
				status: 'unavailable',
				message:
					"This browser can't store sync keys safely. Try another browser.",
			};
		}
		const identity = this.#identity;
		if (!identity) {
			return {
				status: 'locked',
				message: 'Unlock this browser to sync.',
			};
		}
		if (!identity.token) {
			return {
				status: 'not_configured',
				message: 'Set up this browser for sync first.',
			};
		}
		let binding = this.#binding;
		if (!binding) {
			binding = await this.#ensureBinding(input.workspaceId).catch(() => null);
		}
		if (!binding) {
			return {
				status: 'not_configured',
				message: "Sync isn't on for this workspace yet.",
			};
		}
		if (
			binding.objectKeys.size === 0 ||
			!binding.objectKeys.has(binding.objectId)
		) {
			return {
				status: 'not_configured',
				message: "This browser doesn't have the keys for this workspace yet.",
			};
		}
		try {
			await this.#provisionObjects(binding, identity);
			binding = this.#binding ?? binding;
			const objects = await this.#liveObjects(binding);
			const objectKeys = await this.#refreshObjectKeys(binding, identity);
			binding = { ...binding, objects, objectKeys };
			this.#binding = binding;
			const result = await runBrowserSyncReconcile({
				...binding,
				identity,
				onRevoked: () => {
					this.#identity = null;
				},
			});
			this.#error = null;
			return {
				status: 'synced',
				pushed: result.pushed,
				applied: result.applied,
				conflicts: result.conflicts.length,
				cursor: result.cursor,
				skippedUnmanaged: result.skippedUnmanaged,
			};
		} catch (error) {
			if (
				error instanceof BrowserSyncEngineError &&
				error.code === BrowserSyncEngineErrorCode.Revoked
			) {
				this.#identity = null;
				return {
					status: 'revoked',
					message:
						'This browser lost access to this workspace. Lock it, and set it up again once you have access.',
				};
			}
			this.#error = messageOf(error);
			return { status: 'error', message: this.#error };
		}
	}

	/** Decode a durable record's pinned signer keys. */
	#decodedPins(
		record: BrowserSyncBindingRecord | null,
	): Map<string, Uint8Array> {
		const pins = new Map<string, Uint8Array>();
		for (const [deviceId, value] of Object.entries(
			record?.pinnedSigners ?? {},
		)) {
			pins.set(deviceId, decodeBase64(value));
		}
		return pins;
	}

	/** Replace the in-memory binding's pinned signers after an approval change. */
	#applyPins(record: BrowserSyncBindingRecord): void {
		if (!this.#binding) return;
		this.#binding = {
			...this.#binding,
			pinnedSigners: this.#decodedPins(record),
		};
	}

	/**
	 * Every active device with a browser-capable `x25519:` recipient.
	 *
	 * A browser cannot wrap to a native `age1` recipient, and the access policy
	 * requires an envelope for every active device. A non-browser device therefore
	 * blocks browser-first bootstrap: that device must create the workspace and
	 * deliver keys to this browser instead.
	 *
	 * During key provisioning the caller passes `browserOnly: true`, which skips a
	 * native recipient instead of failing, because an already-active native device
	 * already holds an envelope for every existing object. A missing envelope for
	 * an existing object is only ever wrapped to a browser device.
	 */
	async #activeWebDevices(
		workspaceId: string,
		identity: DeviceIdentity,
		options: { state?: AccessState; browserOnly?: boolean } = {},
	): Promise<Array<{ deviceId: string; recipient: string }>> {
		const state =
			options.state ??
			(await accessState({
				origin: this.#origin,
				token: identity.token,
				workspaceId,
				fetch: this.#fetch,
			}));
		const devices: Array<{ deviceId: string; recipient: string }> = [];
		for (const raw of state.devices) {
			if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;
			const row = raw as Record<string, unknown>;
			const deviceId = row.deviceId;
			const recipient = row.encryptionRecipient;
			if (
				typeof deviceId !== 'string' ||
				typeof recipient !== 'string' ||
				recipient.length === 0
			)
				continue;
			if (!recipient.startsWith('x25519:')) {
				if (options.browserOnly) continue;
				throw new BrowserSyncError(
					BrowserSyncErrorCode.IncompatibleRecipient,
					"Another device on your account can't share keys with a browser. Turn on sync from that device instead.",
				);
			}
			devices.push({ deviceId, recipient });
		}
		if (!devices.some((device) => device.deviceId === identity.deviceId))
			devices.push({
				deviceId: identity.deviceId,
				recipient: identity.recipient,
			});
		return devices;
	}

	/**
	 * List the workspace's devices with their locally computed fingerprints.
	 *
	 * The signing public key of each device comes from the server's access state,
	 * but the fingerprint is computed locally and trust is read only from the
	 * pinned signer set. The access state is never added to the pins.
	 */
	async listWorkspaceDevices(
		input: { workspaceId?: string } = {},
	): Promise<BrowserSyncResult<BrowserSyncDeviceCard[]>> {
		const identity = this.#identity;
		if (!identity) {
			return failure('locked', 'Unlock this browser to see your devices.');
		}
		if (!identity.token) {
			return failure('not_configured', 'Set up this browser for sync first.');
		}
		const record = await this.#loadRecord(input.workspaceId).catch(() => null);
		const workspaceId = this.#binding?.workspaceId ?? record?.workspaceId;
		if (!workspaceId) {
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		}
		const pins = this.#binding?.pinnedSigners ?? this.#decodedPins(record);
		try {
			const state = await accessState({
				origin: this.#origin,
				token: identity.token,
				workspaceId,
				fetch: this.#fetch,
			});
			const devices: BrowserSyncDeviceCard[] = [];
			for (const raw of state.devices) {
				if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;
				const row = raw as Record<string, unknown>;
				const deviceId = row.deviceId;
				const accountId = row.accountId;
				const publicKey = row.publicKey;
				const encryptionRecipient = row.encryptionRecipient;
				if (
					typeof deviceId !== 'string' ||
					typeof accountId !== 'string' ||
					typeof publicKey !== 'string' ||
					typeof encryptionRecipient !== 'string' ||
					encryptionRecipient.length === 0
				)
					continue;
				let fingerprint: string;
				try {
					fingerprint = deviceFingerprintForCard(
						deviceId,
						accountId,
						decodeBase64(publicKey, 32),
						encryptionRecipient,
					);
				} catch {
					continue;
				}
				devices.push({
					deviceId,
					accountId,
					publicKey,
					encryptionRecipient,
					fingerprint,
					approved: pins.has(deviceId),
				});
			}
			devices.sort((left, right) =>
				left.deviceId.localeCompare(right.deviceId),
			);
			return ok(devices);
		} catch (error) {
			this.#error = messageOf(error);
			return failure('sync_failed', this.#error);
		}
	}

	/**
	 * Approve a device by storing its signing public key, but only when the
	 * supplied fingerprint matches the locally computed fingerprint for the
	 * device card. A mismatch stores nothing.
	 */
	async approveDevice(
		deviceId: string,
		fingerprint: string,
	): Promise<BrowserSyncResult<BrowserSyncDeviceCard>> {
		if (!deviceId) {
			return failure('device_not_found', 'Choose a device.');
		}
		const identity = this.#identity;
		if (!identity) {
			return failure('locked', 'Unlock this browser to approve a device.');
		}
		const record = await this.#loadRecord().catch(() => null);
		const store = record
			? await this.#resolveBindingStore(record.localWorkspaceId)
			: null;
		if (!record || !store) {
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		}
		const listed = await this.listWorkspaceDevices();
		if (!listed.ok) return failure(listed.code, listed.message);
		const device = listed.value.find((entry) => entry.deviceId === deviceId);
		if (!device) {
			return failure(
				'device_not_found',
				"That device isn't part of this workspace.",
			);
		}
		if (device.fingerprint !== fingerprint) {
			return failure(
				'fingerprint_mismatch',
				"The security code doesn't match that device, so nothing changed.",
			);
		}
		const next: BrowserSyncBindingRecord = {
			...record,
			pinnedSigners: {
				...record.pinnedSigners,
				[deviceId]: device.publicKey,
			},
		};
		await store.write(next);
		this.#record = next;
		this.#applyPins(next);
		return ok({ ...device, approved: true });
	}

	/** Remove a local device approval; the signing key is dropped from the pins. */
	async revokeDeviceApproval(
		deviceId: string,
	): Promise<BrowserSyncResult<boolean>> {
		const record = await this.#loadRecord().catch(() => null);
		const store = record
			? await this.#resolveBindingStore(record.localWorkspaceId)
			: null;
		if (!record || !store) {
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		}
		if (!(deviceId in record.pinnedSigners)) return ok(false);
		const pinnedSigners = { ...record.pinnedSigners };
		delete pinnedSigners[deviceId];
		const next: BrowserSyncBindingRecord = { ...record, pinnedSigners };
		await store.write(next);
		this.#record = next;
		this.#applyPins(next);
		return ok(true);
	}

	/**
	 * Resolve one recorded conflict through the engine and refresh the summary.
	 *
	 * This delegates to {@link runBrowserSyncResolveConflict}; an unknown
	 * operation id is reported as a typed failure and success is never faked.
	 */
	async resolveConflict(
		operationId: string,
		choice: SyncConflictResolution = 'remote',
	): Promise<
		BrowserSyncResult<{
			resolved: BrowserSyncConflictDetail;
			remaining: number;
		}>
	> {
		const identity = this.#identity;
		if (!identity) {
			return failure('locked', 'Unlock this browser to resolve conflicts.');
		}
		let binding = this.#binding;
		if (!binding) {
			binding = await this.#ensureBinding().catch(() => null);
		}
		if (!binding) {
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		}
		try {
			const objects = await this.#liveObjects(binding);
			const result = await runBrowserSyncResolveConflict(
				{
					...binding,
					objects,
					identity,
					onRevoked: () => {
						this.#identity = null;
					},
				},
				operationId,
				choice,
			);
			await this.workspaceSummary();
			return ok({
				resolved: {
					operationId: result.resolved.operationId,
					objectId: result.resolved.objectId,
					path: result.resolved.path,
					reason: result.resolved.reason,
				},
				remaining: result.remaining.length,
			});
		} catch (error) {
			this.#error = messageOf(error);
			return failure('sync_failed', this.#error);
		}
	}

	/**
	 * Resolve the bound object that owns a note's attachments.
	 *
	 * Per-object bindings are matched by the stable local object id first, then
	 * by the note's current path. A single-object binding owns its own
	 * attachments. Returns `null` when the note is unmanaged, so the caller
	 * reports a blocker rather than provisioning a new remote object.
	 */
	async #resolveAttachmentObject(input: {
		noteId?: string;
		notePath?: string;
	}): Promise<BrowserSyncObjectBinding | null> {
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding) return null;
		const objects = binding.objects
			? await this.#liveObjects(binding)
			: undefined;
		if (objects && objects.size > 0) {
			for (const object of objects.values()) {
				if (input.noteId !== undefined && object.localObjectId === input.noteId)
					return object;
				if (
					input.notePath !== undefined &&
					(object.path === input.notePath || object.livePath === input.notePath)
				)
					return object;
			}
			return null;
		}
		if (!binding.objectKeys.has(binding.objectId)) return null;
		return {
			objectId: binding.objectId,
			path: input.notePath ?? '',
			epoch: binding.epoch,
			policyRevision: binding.policyRevision,
		};
	}

	/** List the attachments stored for one note's owning object. */
	async listNoteAttachments(input: {
		noteId?: string;
		notePath?: string;
	}): Promise<BrowserSyncResult<Array<{ path: string; name: string }>>> {
		const identity = this.#identity;
		if (!identity)
			return failure('locked', 'Unlock this browser to see attachments.');
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		const target = await this.#resolveAttachmentObject(input);
		if (!target)
			return failure('object_not_found', "This note isn't synced yet.");
		try {
			const prefix = `${ATTACHMENT_ROOT}/${target.objectId}/`;
			const paths = (await binding.storage.list()).filter(
				(path) =>
					path.startsWith(prefix) && isAttachmentPathFor(target.objectId, path),
			);
			const items = paths
				.map((path) => ({
					path,
					name: attachmentNameFromPath(path) ?? path.slice(prefix.length),
				}))
				.sort((left, right) => left.name.localeCompare(right.name));
			return ok(items);
		} catch (error) {
			this.#error = messageOf(error);
			return failure('sync_failed', this.#error);
		}
	}

	/** Read one attachment's plaintext bytes from the local replica. */
	async readNoteAttachment(input: {
		noteId?: string;
		notePath?: string;
		path: string;
	}): Promise<
		BrowserSyncResult<{ name: string; bytes: Uint8Array<ArrayBuffer> }>
	> {
		const identity = this.#identity;
		if (!identity)
			return failure('locked', 'Unlock this browser to download attachments.');
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		const target = await this.#resolveAttachmentObject(input);
		if (!target || !isAttachmentPathFor(target.objectId, input.path))
			return failure(
				'object_not_found',
				"That file isn't attached to this note.",
			);
		try {
			const file = await binding.storage.read(input.path);
			if (file === null)
				return failure(
					'attachment_unavailable',
					"That file isn't in this browser yet. Sync, then try again.",
				);
			return ok({
				name: attachmentNameFromPath(input.path) ?? input.path,
				bytes: new Uint8Array(file.bytes),
			});
		} catch (error) {
			this.#error = messageOf(error);
			return failure('attachment_failed', this.#error);
		}
	}

	/**
	 * Encrypt, upload, and queue one attachment for the note's owning object.
	 *
	 * Nothing is enqueued unless the server acknowledges the complete ciphertext
	 * and the canonical bytes reach the local replica. Returns the queued path and
	 * descriptor; the caller reconciles to publish the operation.
	 */
	async sendAttachment(input: {
		noteId?: string;
		notePath?: string;
		name: string;
		bytes: Uint8Array;
		onProgress?: (uploadedBytes: number, totalBytes: number) => void;
	}): Promise<BrowserSyncResult<BrowserSyncSendAttachmentOutcome>> {
		if (!this.#keyStore)
			return failure(
				'unavailable',
				"This browser can't store sync keys safely. Try another browser.",
			);
		const identity = this.#identity;
		if (!identity)
			return failure('locked', 'Unlock this browser to attach files.');
		if (!identity.token)
			return failure('not_configured', 'Set up this browser for sync first.');
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure('not_configured', "Sync isn't on for this workspace yet.");
		if (!binding.attachments)
			return failure(
				'not_configured',
				"Sync isn't connected, so you can't upload files yet.",
			);
		const target = await this.#resolveAttachmentObject(input);
		if (!target)
			return failure(
				'object_not_found',
				"This note isn't synced yet. Sync, then try again.",
			);
		if (input.bytes.length > MAX_BROWSER_ATTACHMENT_BYTES)
			return failure(
				'attachment_too_large',
				`Attachments are limited to ${Math.floor(
					MAX_BROWSER_ATTACHMENT_BYTES / (1024 * 1024),
				)} MB in the browser. This file is ${Math.ceil(
					input.bytes.length / (1024 * 1024),
				)} MB.`,
			);
		try {
			const result = await runBrowserSyncSendAttachment({
				...binding,
				identity,
				objectId: target.objectId,
				name: input.name,
				bytes: input.bytes,
				...(input.onProgress === undefined
					? {}
					: { onProgress: input.onProgress }),
				onRevoked: () => {
					this.#identity = null;
				},
			});
			return ok(result);
		} catch (error) {
			if (
				error instanceof BrowserSyncEngineError &&
				error.code === BrowserSyncEngineErrorCode.Revoked
			) {
				this.#identity = null;
				return failure('locked', 'This browser lost access to this workspace.');
			}
			this.#error = messageOf(error);
			if (
				error instanceof BrowserSyncError &&
				error.code === BrowserSyncErrorCode.BlobTooLarge
			)
				return failure('attachment_too_large', this.#error);
			if (
				error instanceof BrowserSyncError &&
				error.code === BrowserSyncErrorCode.InvalidOperation
			)
				return failure('object_not_found', this.#error);
			return failure('attachment_failed', this.#error);
		}
	}

	/**
	 * Attach one file to a note and publish it with a sync pass.
	 *
	 * A note created since the last sync has no remote object yet. When the
	 * first upload reports `object_not_found`, one sync pass provisions the
	 * object and the upload is retried once. `onUploaded` runs after the
	 * ciphertext is stored and before the publishing sync pass.
	 */
	async attachAndSync(input: {
		noteId?: string;
		notePath?: string;
		name: string;
		bytes: Uint8Array;
		onProgress?: (uploadedBytes: number, totalBytes: number) => void;
		onUploaded?: () => void;
	}): Promise<
		BrowserSyncResult<{
			attachment: BrowserSyncSendAttachmentOutcome;
			sync: SyncNowOutcome;
		}>
	> {
		const { onUploaded, ...send } = input;
		let sent = await this.sendAttachment(send);
		if (!sent.ok && sent.code === 'object_not_found') {
			await this.syncNow();
			sent = await this.sendAttachment(send);
		}
		if (!sent.ok) return sent;
		onUploaded?.();
		const sync = await this.syncNow();
		return ok({ attachment: sent.value, sync });
	}
}

function enrollFailureCode(error: unknown): BrowserSyncFailureCode {
	if (error instanceof BrowserSyncError) {
		if (error.code === BrowserSyncErrorCode.PassphraseRejected)
			return 'passphrase_rejected';
	}
	return 'enroll_failed';
}

/**
 * Create a controller. Always resolves: an unsupported browser yields a
 * controller whose `status()` is `unavailable` rather than a thrown error.
 */
export async function createBrowserSyncController(
	options: BrowserSyncControllerOptions = {},
): Promise<BrowserSyncController> {
	const keyStore =
		options.keyStore !== undefined
			? options.keyStore
			: await openBrowserSyncKeyStore();
	const origin = options.origin ?? globalThis.location?.origin ?? '';
	const fetchImpl = options.fetch ?? createSameOriginFetch(origin);
	const controller = new BrowserSyncController({
		bundleId: options.bundleId ?? DEFAULT_BROWSER_SYNC_BUNDLE_ID,
		origin,
		keyStore,
		fetch: fetchImpl,
		binding: options.binding ?? null,
		workspaceFiles: options.workspaceFiles ?? null,
		bindingStore: options.bindingStore,
		stateStore: options.stateStore,
	});
	await controller.init();
	return controller;
}
