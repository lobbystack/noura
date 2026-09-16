/**
 * Host wiring for browser device custody and encrypted workspace sync.
 *
 * This module is the `apps/app` glue over `@noura/browser-sync` (custody,
 * enrollment, key delivery, operation transport, file-change codec) and
 * `@noura/browser-sync-engine` (local replica reconciliation). It is
 * deliberately not a Svelte module: the UI imports it from `onMount` so no
 * browser-only global runs during SSR or prerender.
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
	attachmentObjectIdFromPath,
	attachmentPath,
	BrowserSyncError,
	BrowserSyncErrorCode,
	BrowserSyncTransport,
	bytesEqual,
	createDeviceIdentity,
	createFileChangeCodec,
	decodeBase64,
	decodeRecipient,
	decryptAttachment,
	deviceFingerprintForCard,
	downloadBlob,
	encodeBase64,
	encryptAttachmentToSink,
	ensureResponseOk,
	enrollBrowserDevice,
	exportRecoveryKit as buildRecoveryKit,
	importRecoveryKit as openRecoveryKit,
	isAttachmentPathFor,
	isBrowserSyncBindingRecord,
	isIdentifier,
	MemoryAttachmentSink,
	NATIVE_RECOVERY_DOMAIN,
	randomBytes,
	randomIdentifier,
	readJson,
	receiveKeys,
	recoverNativeKeysToBrowserBinding as rewrapNativeKeys,
	requestDeviceChallenge,
	sealIdentity,
	signAccessPolicy,
	unlockDeviceIdentity,
	unwrapKey,
	uploadBlob,
	viewAttachmentSource,
	wrapKey,
	type AccessPolicy,
	type AccessPolicyEnvelope,
	type AccessPolicyObject,
	type AccessState,
	type AttachmentSource,
	type BrowserSyncBindingRecord,
	type BrowserSyncBoundKey,
	type BrowserSyncBoundObject,
	type DeviceIdentity,
	type FetchLike,
	type FileChangeBlob,
	type KeyStore,
	type NativeRecoveryEnvelope,
	type NativeRecoveryObject,
	type RecoverNativeKeysToBrowserBindingInput,
	type RecoveryKitFile,
	type WebKeyEnvelope,
	type WrappedKeyBundle,
} from '@noura/browser-sync';
import type {
	BrowserWorkspaceFiles,
	BrowserWorkspaceObjectCard,
} from '@noura/browser-workspace';
import {
	BrowserSyncEngine,
	BrowserSyncEngineError,
	BrowserSyncEngineErrorCode,
	createFileSystemSyncStateStore,
	type AttachmentFetcher,
	type BrowserSyncEngineOptions,
	type BrowserSyncRemote,
	type BrowserSyncStorage,
	type FileChange,
	type FileChangeCodec as EngineFileChangeCodec,
	type ReconcileResult,
	type ResolveConflictResult,
	type SyncConflictResolution,
	type SyncState,
	type SyncStateStore,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import { OpfsFileSystem } from '@noura/browser-storage';

/** Directory outside canonical workspace files that holds adapter-owned state. */
const ADAPTER_DIRECTORY = '.noura-adapter/browser-sync';

/** Default stable key for this browser's wrapped device bundle. */
export const DEFAULT_BROWSER_SYNC_BUNDLE_ID = 'browser-device';

/** Roots the sync protocol reserves; they never appear as file changes. */
const RESERVED_SYNC_ROOTS = new Set([
	'.noura',
	'.git',
	'node_modules',
	'target',
]);

/** True when a canonical path is eligible for an encrypted file change. */
export function isSyncablePath(path: string): boolean {
	const first = path.split('/')[0] ?? '';
	return path.length > 0 && !RESERVED_SYNC_ROOTS.has(first.toLowerCase());
}

/**
 * The binding record types are owned by `@noura/browser-sync` so the recovery
 * kit and the storage path validate the same shape. They are re-exported here
 * for existing consumers of this module.
 */
export type {
	BrowserSyncBindingRecord,
	BrowserSyncBoundKey,
	BrowserSyncBoundObject,
} from '@noura/browser-sync';

/** Durable store for one workspace's binding record, outside canonical files. */
export interface BrowserSyncBindingStore {
	read(): Promise<BrowserSyncBindingRecord | null>;
	write(record: BrowserSyncBindingRecord): Promise<void>;
	remove(): Promise<boolean>;
}

function cloneBindingRecord(
	record: BrowserSyncBindingRecord,
): BrowserSyncBindingRecord {
	const objects: Record<string, BrowserSyncBoundObject> = {};
	for (const [objectId, bound] of Object.entries(record.objects)) {
		objects[objectId] = { ...bound, key: { ...bound.key } };
	}
	return {
		...record,
		objects,
		pinnedSigners: { ...record.pinnedSigners },
	};
}

/** In-memory binding store for tests. Not durable. */
export function createMemoryBindingStore(
	initial?: BrowserSyncBindingRecord,
): BrowserSyncBindingStore {
	let record = initial ? cloneBindingRecord(initial) : null;
	return {
		async read() {
			return record ? cloneBindingRecord(record) : null;
		},
		async write(next) {
			record = cloneBindingRecord(next);
		},
		async remove() {
			const had = record !== null;
			record = null;
			return had;
		},
	};
}

function parseBindingRecord(text: string): BrowserSyncBindingRecord {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser sync binding was not valid JSON',
		);
	}
	if (!isBrowserSyncBindingRecord(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser sync binding had an unexpected shape',
		);
	}
	return value;
}

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

/** True when access-state lists an active device with a non-browser recipient. */
function hasNativeActiveDevice(state: AccessState): boolean {
	for (const raw of state.devices) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;
		const recipient = (raw as Record<string, unknown>).encryptionRecipient;
		if (
			typeof recipient === 'string' &&
			recipient.length > 0 &&
			!recipient.startsWith('x25519:')
		)
			return true;
	}
	return false;
}

/**
 * Parse the server's latest signed access policy from an access-state response.
 *
 * Returns `null` when no policy exists yet. A present policy must match the
 * browser's {@link AccessPolicy} shape closely enough that rebuilding and
 * re-signing it preserves the server's member, grant, document, and envelope
 * fields; an unexpected shape returns `null` so provisioning is skipped rather
 * than fabricating a policy.
 */
function parseAccessPolicy(value: unknown): AccessPolicy | null {
	if (value === null || value === undefined) return null;
	if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
	const policy = value as Record<string, unknown>;
	const version = policy.version;
	if (version !== 1 && version !== 2) return null;
	if (
		typeof policy.workspaceId !== 'string' ||
		typeof policy.revision !== 'string' ||
		(policy.previousPolicyDigest !== null &&
			typeof policy.previousPolicyDigest !== 'string') ||
		typeof policy.deviceId !== 'string' ||
		typeof policy.signature !== 'string' ||
		!Array.isArray(policy.members) ||
		!Array.isArray(policy.objects)
	)
		return null;
	const members: AccessPolicy['members'] = [];
	for (const raw of policy.members) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
		const member = raw as Record<string, unknown>;
		if (
			typeof member.accountId !== 'string' ||
			(member.role !== 'owner' &&
				member.role !== 'admin' &&
				member.role !== 'editor' &&
				member.role !== 'viewer')
		)
			return null;
		members.push({
			accountId: member.accountId,
			role: member.role as AccessPolicy['members'][number]['role'],
		});
	}
	const objects: AccessPolicyObject[] = [];
	for (const raw of policy.objects) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
		const object = raw as Record<string, unknown>;
		if (
			typeof object.objectId !== 'string' ||
			!Number.isSafeInteger(object.epoch) ||
			(object.epoch as number) < 1 ||
			!Array.isArray(object.grants) ||
			!Array.isArray(object.envelopes)
		)
			return null;
		const grants: AccessPolicyObject['grants'] = [];
		for (const rawGrant of object.grants) {
			if (!rawGrant || typeof rawGrant !== 'object' || Array.isArray(rawGrant))
				return null;
			const grant = rawGrant as Record<string, unknown>;
			if (
				typeof grant.accountId !== 'string' ||
				(grant.role !== 'editor' && grant.role !== 'viewer')
			)
				return null;
			grants.push({
				accountId: grant.accountId,
				role: grant.role as AccessPolicyObject['grants'][number]['role'],
			});
		}
		const envelopes: AccessPolicyEnvelope[] = [];
		for (const rawEnvelope of object.envelopes) {
			if (
				!rawEnvelope ||
				typeof rawEnvelope !== 'object' ||
				Array.isArray(rawEnvelope)
			)
				return null;
			const envelope = rawEnvelope as Record<string, unknown>;
			if (
				typeof envelope.deviceId !== 'string' ||
				typeof envelope.wrappedKey !== 'string' ||
				typeof envelope.signature !== 'string'
			)
				return null;
			const parsed: AccessPolicyEnvelope = {
				deviceId: envelope.deviceId,
				wrappedKey: envelope.wrappedKey,
				signature: envelope.signature,
			};
			if (envelope.construction === 'web' || envelope.construction === 'age')
				parsed.construction = envelope.construction;
			if (typeof envelope.recipientPublicKey === 'string')
				parsed.recipientPublicKey = envelope.recipientPublicKey;
			if (typeof envelope.ephemeralPublicKey === 'string')
				parsed.ephemeralPublicKey = envelope.ephemeralPublicKey;
			if (typeof envelope.salt === 'string') parsed.salt = envelope.salt;
			if (typeof envelope.nonce === 'string') parsed.nonce = envelope.nonce;
			envelopes.push(parsed);
		}
		let document: AccessPolicyObject['document'];
		if (
			object.document &&
			typeof object.document === 'object' &&
			!Array.isArray(object.document)
		) {
			const descriptor = object.document as Record<string, unknown>;
			if (
				typeof descriptor.generation === 'string' &&
				(descriptor.mode === 'text' || descriptor.mode === 'attachment')
			)
				document = {
					generation: descriptor.generation,
					mode: descriptor.mode,
				};
		}
		// A version-2 policy binds a document descriptor per object; the server
		// rejects a version-2 object without one. A malformed or partial response
		// must not be rebuilt and re-signed, so skip provisioning instead.
		if (version === 2 && document === undefined) return null;
		objects.push({
			objectId: object.objectId,
			epoch: object.epoch as number,
			grants,
			envelopes,
			...(document === undefined ? {} : { document }),
		});
	}
	return {
		version,
		workspaceId: policy.workspaceId,
		revision: policy.revision,
		previousPolicyDigest: policy.previousPolicyDigest as string | null,
		deviceId: policy.deviceId,
		members,
		objects,
		signature: policy.signature,
	};
}

/** Controller custody and availability state. */
export type BrowserSyncStatus =
	'unavailable' | 'locked' | 'unlocked' | 'enrolled' | 'error';

/** Public, non-secret device projection. */
export interface BrowserSyncDeviceInfo {
	deviceId: string;
	enrolled: boolean;
}

/** Failure discriminator for controller actions. */
export type BrowserSyncFailureCode =
	| 'unavailable'
	| 'not_configured'
	| 'locked'
	| 'invalid_passphrase'
	| 'passphrase_rejected'
	| 'enroll_failed'
	| 'custody_failed'
	| 'device_not_found'
	| 'fingerprint_mismatch'
	| 'sync_failed'
	| 'attachment_failed'
	| 'attachment_too_large'
	| 'attachment_unavailable'
	| 'object_not_found';

/** Uniform typed result for custody actions. */
export type BrowserSyncResult<T = undefined> =
	| { ok: true; value: T }
	| { ok: false; code: BrowserSyncFailureCode; message: string };

/** Outcome of {@link BrowserSyncController.syncNow}. */
export type SyncNowOutcome =
	| {
			status: 'synced';
			pushed: number;
			applied: number;
			conflicts: number;
			cursor: string;
			/** Local files with no owning object; skipped, never sealed under another object. */
			skippedUnmanaged: number;
	  }
	| { status: 'unavailable'; message: string }
	| { status: 'locked'; message: string }
	| { status: 'not_configured'; message: string }
	| { status: 'revoked'; message: string }
	| { status: 'error'; message: string };

/** One recorded conflict, projected for the workspace summary. */
export interface BrowserSyncConflictDetail {
	operationId: string;
	objectId: string;
	path: string;
	reason: string;
}

/** Read-only counters for the currently bound workspace replica. */
export interface BrowserSyncWorkspaceSummary {
	configured: boolean;
	workspaceId: string | null;
	cursor: string;
	pending: number;
	conflicts: number;
	/** Recorded conflicts with enough detail to choose a resolution. */
	conflictDetails: BrowserSyncConflictDetail[];
}

/** Public, non-secret projection of one device in a workspace access state. */
export interface BrowserSyncDeviceCard {
	deviceId: string;
	accountId: string;
	publicKey: string;
	encryptionRecipient: string;
	fingerprint: string;
	approved: boolean;
}

/** One remote sync object bound to this browser workspace. */
export interface BrowserSyncObjectBinding {
	/** Stable remote object ID this entry wraps a key for. */
	objectId: string;
	/** Canonical path the object owned when it was bound. */
	path: string;
	/** Stable local object ID, used to follow a move to a new path. */
	localObjectId?: string;
	/** Current local path from the managed-object list, when it differs from `path`. */
	livePath?: string;
	/**
	 * True when this object was recovered from a native kit and has no verified
	 * canonical local path. Unmapped objects can open delivered operations but
	 * never own a local file or attachment.
	 */
	unmapped?: boolean;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical decimal access-policy revision the object was bound at. */
	policyRevision: string;
}

/**
 * The injectable boundaries one browser workspace replica needs. A host binds
 * these once `BrowserWorkspaceStorage` is reachable from the same thread as the
 * controller (today it is not, so the hosted UI leaves this unset).
 */
export interface BrowserSyncWorkspaceBinding {
	/** Stable workspace id. */
	workspaceId: string;
	/**
	 * Primary/default object used when {@link objects} is absent. Kept so a
	 * single-object binding (including one persisted before per-object sync)
	 * keeps working unchanged.
	 */
	objectId: string;
	/** Positive safe-integer key epoch of the primary object. */
	epoch: number;
	/** Canonical decimal access-policy revision of the primary object. */
	policyRevision: string;
	/** Object keys by object id; a key must exist for `objectId`. */
	objectKeys: ReadonlyMap<string, Uint8Array>;
	/**
	 * Per-object bindings by object id. When present and non-empty, a file change
	 * is sealed under the object whose current or last-known path matches. When
	 * absent, every change is sealed under the primary `objectId`.
	 */
	objects?: ReadonlyMap<string, BrowserSyncObjectBinding>;
	/** Pinned Ed25519 public keys by signing device id. */
	pinnedSigners: ReadonlyMap<string, Uint8Array>;
	/** Local replica boundary. */
	storage: BrowserSyncStorage;
	/** Durable state boundary. */
	state: SyncStateStore;
	/** Encrypted remote boundary. */
	remote: BrowserSyncRemote;
	/** Clock used to timestamp conflicts. */
	now?: () => number;
	/**
	 * Transport metadata used to download and decrypt version-3 attachment
	 * ciphertext during reconcile. Absent when the host did not supply an origin
	 * and token; a version-3 change is then refused rather than written empty.
	 */
	attachments?: {
		origin: string;
		token: string;
		fetch: FetchLike;
	};
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

/** Inputs for {@link runBrowserSyncReconcile}. */
export interface BrowserSyncReconcileInput extends BrowserSyncWorkspaceBinding {
	/** Unlocked device identity holding the signing seed. */
	identity: DeviceIdentity;
	/** Called once if revocation locks the engine, so the host can clear keys. */
	onRevoked?: (error: unknown) => void | Promise<void>;
}

function messageOf(error: unknown): string {
	if (error instanceof Error && error.message) return error.message;
	if (
		error &&
		typeof error === 'object' &&
		'message' in error &&
		typeof (error as { message?: unknown }).message === 'string'
	) {
		return String((error as { message: string }).message);
	}
	return 'The browser sync operation failed.';
}

function ok<T>(value: T): BrowserSyncResult<T> {
	return { ok: true, value };
}

function failure<T = undefined>(
	code: BrowserSyncFailureCode,
	message: string,
): BrowserSyncResult<T> {
	return { ok: false, code, message };
}

/**
 * Only same-origin requests are allowed. The hosted app is served by the sync
 * server, so a relative URL and the page origin address the same service.
 *
 * Redirects are refused (`redirect: 'error'`): checking only the initial URL
 * would let a malicious sync server answer with a 30x to an arbitrary origin.
 */
export function createSameOriginFetch(
	base: string = globalThis.location?.origin ?? '',
	fetchImpl: FetchLike = globalThis.fetch,
): FetchLike {
	return (input, init) => {
		if (base && !isSameOrigin(input, base)) {
			return Promise.reject(
				new BrowserSyncError(
					BrowserSyncErrorCode.RequestFailed,
					'Browser sync only uses same-origin requests',
				),
			);
		}
		return fetchImpl(input, { ...init, redirect: 'error' });
	};
}

function isSameOrigin(input: RequestInfo | URL, base: string): boolean {
	try {
		const resolved =
			typeof input === 'string'
				? new URL(input, base)
				: input instanceof URL
					? input
					: new URL(input.url, base);
		return resolved.origin === new URL(base).origin;
	} catch {
		return false;
	}
}

/**
 * Build the engine's remote boundary from the bearer-token transport. Kept here
 * so the same wiring is reused when a host can bind a workspace replica.
 */
export function createBrowserSyncRemote(options: {
	origin: string;
	token: string;
	workspaceId: string;
	fetch?: FetchLike;
}): BrowserSyncRemote {
	const transport = new BrowserSyncTransport({
		origin: options.origin,
		token: options.token,
		fetch: options.fetch ?? createSameOriginFetch(options.origin),
	});
	return {
		async push(operations) {
			const sequences = await transport.push(options.workspaceId, operations);
			return { sequences };
		},
		pull(cursor) {
			return transport.pull(options.workspaceId, cursor);
		},
	};
}

/**
 * Largest attachment this browser will download and decrypt into memory before
 * handing the plaintext to the workspace storage boundary. This is well below
 * the protocol's 1 GiB limit because the browser has no streaming decrypt path
 * into OPFS yet; larger attachments are refused with a structured error.
 */
export const MAX_BROWSER_ATTACHMENT_BYTES = 64 * 1024 * 1024;

/**
 * Build the engine's attachment fetcher over the sync transport.
 *
 * A version-3 change is downloaded as bounded ciphertext ranges, verified
 * against its signed SHA-256 digest, decrypted with the object key recovered for
 * `change.objectId`, and returned only when the plaintext length matches the
 * signed descriptor. Any missing key, unavailable blob, oversized blob, or
 * failed authentication throws so the engine never writes an empty file.
 */
export function createBrowserAttachmentFetcher(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	objectKeys: ReadonlyMap<string, Uint8Array>;
	maxBytes?: number;
}): AttachmentFetcher {
	const maxBytes = input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES;
	return {
		async fetch(change) {
			const blob = change.blob;
			if (!blob) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidBlob,
					'An attachment fetch was requested for a change without a blob.',
				);
			}
			if (blob.size > maxBytes || blob.plaintextSize > maxBytes) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.BlobTooLarge,
					'This attachment is larger than the browser can decrypt in memory.',
				);
			}
			const objectKey = input.objectKeys.get(change.objectId);
			if (!objectKey) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.MissingKey,
					'No object key is available for the attachment object.',
				);
			}
			const sink = new MemoryAttachmentSink();
			await downloadBlob({
				origin: input.origin,
				token: input.token,
				fetch: input.fetch,
				workspaceId: input.workspaceId,
				objectId: change.objectId,
				epoch: change.epoch,
				blob,
				sink,
			});
			return decryptAttachment(objectKey, blob, sink.toBytes());
		},
	};
}

/** Outcome of one reconcile pass, including unmanaged files that were skipped. */
export interface BrowserSyncReconcileOutcome extends ReconcileResult {
	/** Local files with no owning object; skipped, never sealed under another object. */
	skippedUnmanaged: number;
}

/**
 * Resolve the object that owns a file change.
 *
 * An attachment path of the form `attachments/<objectId>/<name>` belongs to the
 * object named in the path, so the containing note's key seals it without a new
 * remote object. Otherwise, when the binding carries per-object entries, the
 * owner is the entry whose current path matches the change path (or a move's
 * source). When it does not, the single primary object owns every change. A
 * change whose path matches no object is unmanaged and returns `null`.
 */
function resolveObjectOwner(
	input: BrowserSyncWorkspaceBinding,
	change: Pick<FileChange, 'path' | 'previousPath'>,
): BrowserSyncObjectBinding | null {
	const attachmentId =
		attachmentObjectIdFromPath(change.path) ??
		(change.previousPath === null
			? null
			: attachmentObjectIdFromPath(change.previousPath));
	if (attachmentId !== null) {
		const objects = input.objects;
		if (objects && objects.size > 0) {
			const owner = objects.get(attachmentId) ?? null;
			return owner && owner.unmapped !== true ? owner : null;
		}
		if (attachmentId !== input.objectId) return null;
		return {
			objectId: input.objectId,
			path: change.path,
			epoch: input.epoch,
			policyRevision: input.policyRevision,
		};
	}
	const objects = input.objects;
	if (objects && objects.size > 0) {
		const owns = (object: BrowserSyncObjectBinding, path: string): boolean =>
			object.unmapped !== true &&
			(object.path === path || object.livePath === path);
		for (const object of objects.values()) {
			if (owns(object, change.path)) return object;
		}
		if (change.previousPath !== null) {
			for (const object of objects.values()) {
				if (owns(object, change.previousPath)) return object;
			}
		}
		return null;
	}
	return {
		objectId: input.objectId,
		path: change.path,
		epoch: input.epoch,
		policyRevision: input.policyRevision,
	};
}

/**
 * Build the per-object file-change codec over the binding.
 *
 * Sealing routes a change to the object that owns its path and seals under that
 * object's key, epoch, and policy revision. Opening resolves by
 * `operation.objectId`, which the base codec already uses to select the key.
 */
function createReconcileCodec(
	input: BrowserSyncReconcileInput,
): EngineFileChangeCodec {
	const pinnedSigners = new Map<string, string>();
	for (const [deviceId, key] of input.pinnedSigners) {
		pinnedSigners.set(deviceId, encodeBase64(key));
	}
	const baseCodec = createFileChangeCodec({
		identity: input.identity,
		objectKeys: input.objectKeys,
		pinnedSigners,
	});
	return {
		sealFileChange(change) {
			const owner = resolveObjectOwner(input, change);
			if (!owner) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidOperation,
					'No sync object owns this file change',
				);
			}
			return baseCodec.sealFileChange({
				workspaceId: input.workspaceId,
				objectId: owner.objectId,
				epoch: owner.epoch,
				policyRevision: owner.policyRevision,
				change,
			});
		},
		async openFileChange(operation) {
			const opened = await baseCodec.openFileChange(operation);
			return {
				path: opened.path,
				previousPath: opened.previousPath,
				baseRevision: opened.baseRevision,
				content: opened.content,
				workspaceId: opened.workspaceId,
				objectId: opened.objectId,
				epoch: opened.epoch,
				...(opened.blob === undefined ? {} : { blob: opened.blob }),
			};
		},
	};
}

function createReconcileEngine(input: BrowserSyncReconcileInput) {
	const attachmentFetcher = input.attachments
		? createBrowserAttachmentFetcher({
				origin: input.attachments.origin,
				token: input.attachments.token,
				fetch: input.attachments.fetch,
				workspaceId: input.workspaceId,
				objectKeys: input.objectKeys,
			})
		: undefined;
	const engineOptions: BrowserSyncEngineOptions = {
		storage: input.storage,
		remote: input.remote,
		codec: createReconcileCodec(input),
		state: input.state,
		deviceId: input.identity.deviceId,
		...(input.now === undefined ? {} : { now: input.now }),
		...(input.onRevoked === undefined ? {} : { onRevoked: input.onRevoked }),
		...(attachmentFetcher === undefined ? {} : { attachmentFetcher }),
	};
	return new BrowserSyncEngine(engineOptions);
}

/**
 * Seal and apply one reconcile pass over the injected boundaries.
 *
 * Local changes are snapshotted, sealed into the durable outbox, and flushed by
 * the engine before it pulls and applies remote operations. This performs
 * cryptography through `@noura/browser-sync`'s file-change codec but no network
 * I/O of its own. A local path with no owning object or no object key is skipped
 * and counted in `skippedUnmanaged`; it is never sealed under another object.
 */
export async function runBrowserSyncReconcile(
	input: BrowserSyncReconcileInput,
): Promise<BrowserSyncReconcileOutcome> {
	const engine = createReconcileEngine(input);
	const changes = await engine.snapshotLocalChanges();
	let skippedUnmanaged = 0;
	for (const change of changes) {
		const owner = resolveObjectOwner(input, change);
		if (!owner || !input.objectKeys.has(owner.objectId)) {
			skippedUnmanaged += 1;
			continue;
		}
		await engine.enqueueFileChange(change);
	}
	const result = await engine.reconcile();
	return { ...result, skippedUnmanaged };
}

/**
 * Resolve one recorded conflict through the same engine boundaries used for
 * reconcile. Returns the engine's typed result; an unknown operation id is
 * rejected with `ConflictNotFound` and success is never fabricated.
 */
export async function runBrowserSyncResolveConflict(
	input: BrowserSyncReconcileInput,
	operationId: string,
	choice: SyncConflictResolution,
): Promise<ResolveConflictResult> {
	const engine = createReconcileEngine(input);
	return engine.resolveConflict(operationId, choice);
}

/** Insert a short suffix before a file name's extension. */
function withUniqueSuffix(name: string, suffix: string): string {
	const dot = name.lastIndexOf('.');
	if (dot > 0) return `${name.slice(0, dot)}-${suffix}${name.slice(dot)}`;
	return `${name}-${suffix}`;
}

/** Choose an attachment path that is absent from the local replica. */
async function uniqueAttachmentPath(
	storage: BrowserSyncStorage,
	objectId: string,
	name: string,
): Promise<string> {
	const base = attachmentPath(objectId, name);
	if ((await storage.read(base)) === null) return base;
	for (let attempt = 0; attempt < 8; attempt += 1) {
		const candidate = attachmentPath(
			objectId,
			withUniqueSuffix(name, randomIdentifier().slice(0, 6)),
		);
		if ((await storage.read(candidate)) === null) return candidate;
	}
	throw new BrowserSyncError(
		BrowserSyncErrorCode.InvalidOperation,
		'Could not choose a unique attachment path',
	);
}

/** Wrap a ciphertext source so each read reports absolute upload progress. */
function progressAttachmentSource(
	source: AttachmentSource,
	onProgress: (uploadedBytes: number, totalBytes: number) => void,
): AttachmentSource {
	return {
		size: source.size,
		async read(offset, length) {
			const bytes = await source.read(offset, length);
			onProgress(offset + bytes.length, source.size);
			return bytes;
		},
	};
}

/** Inputs for {@link runBrowserSyncSendAttachment}. */
export interface BrowserSyncSendAttachmentInput extends BrowserSyncReconcileInput {
	/** Object that owns the attachment; the containing note's object. */
	objectId: string;
	/** User-supplied file name, reduced to one portable path component. */
	name: string;
	/** Complete plaintext bytes. */
	bytes: Uint8Array;
	/** Called as ciphertext reaches the server, in bounded upload steps. */
	onProgress?: (uploadedBytes: number, totalBytes: number) => void;
	/**
	 * Largest plaintext accepted, defaulting to
	 * {@link MAX_BROWSER_ATTACHMENT_BYTES}. Injectable so tests can exercise the
	 * bound without allocating a large buffer.
	 */
	maxBytes?: number;
}

/** Result of a successful {@link runBrowserSyncSendAttachment}. */
export interface BrowserSyncSendAttachmentOutcome {
	/** Workspace path the version-3 change writes. */
	path: string;
	/** Signed encrypted descriptor carried by the queued change. */
	blob: FileChangeBlob;
}

/**
 * Attach one file to a managed object without a network round trip of its own
 * beyond the blob upload.
 *
 * The attachment is encrypted with the owning object's key using the same `age`
 * v1 construction as native, uploaded through the bounded resumable path (which
 * only resolves once the server acknowledges completion), written to the local
 * replica at `attachments/<objectId>/<name>`, then sealed and enqueued as a
 * version-3 file change. No operation is enqueued if encryption, upload, or the
 * local write fails. The caller reconciles to push the queued operation.
 */
export async function runBrowserSyncSendAttachment(
	input: BrowserSyncSendAttachmentInput,
): Promise<BrowserSyncSendAttachmentOutcome> {
	if (!input.attachments) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.RequestFailed,
			'No sync transport is configured, so an attachment cannot be uploaded.',
		);
	}
	if (input.bytes.length > (input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobTooLarge,
			`Attachments are limited to ${input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES} bytes in the browser.`,
		);
	}
	// Validate the name and resolve the owner before any encryption or upload.
	const candidate = attachmentPath(input.objectId, input.name);
	const owner = resolveObjectOwner(input, {
		path: candidate,
		previousPath: null,
	});
	if (!owner || owner.objectId !== input.objectId) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'The containing object is not bound for sync, so the attachment cannot be attached.',
		);
	}
	const objectKey = input.objectKeys.get(owner.objectId);
	if (!objectKey) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.MissingKey,
			'No object key is available for the containing object.',
		);
	}
	const path = await uniqueAttachmentPath(
		input.storage,
		owner.objectId,
		input.name,
	);

	const sink = new MemoryAttachmentSink();
	const blob = await encryptAttachmentToSink(
		objectKey,
		viewAttachmentSource(input.bytes),
		sink,
	);
	const ciphertext = sink.toBytes();
	await uploadBlob({
		origin: input.attachments.origin,
		token: input.attachments.token,
		fetch: input.attachments.fetch,
		workspaceId: input.workspaceId,
		objectId: owner.objectId,
		epoch: owner.epoch,
		blob,
		source:
			input.onProgress === undefined
				? viewAttachmentSource(ciphertext)
				: progressAttachmentSource(
						viewAttachmentSource(ciphertext),
						input.onProgress,
					),
	});

	// The blob is complete. Write the canonical bytes locally so the replica is
	// consistent, then enqueue the change. A failed seal must not leave a local
	// file that a later snapshot would seal as inline content.
	await input.storage.write({
		path,
		bytes: input.bytes,
		expectedRevision: null,
	});
	const engine = createReconcileEngine(input);
	try {
		await engine.enqueueFileChange({
			path,
			previousPath: null,
			baseRevision: null,
			content: null,
			blob,
		});
	} catch (error) {
		await input.storage.delete({ path }).catch(() => {});
		throw error;
	}
	return { path, blob };
}

/**
 * The subset of `StorageManager` this module uses. `getDirectory` is optional
 * because it is not present in every browser or type library.
 */
/** Inputs for {@link createBrowserSyncWorkspaceBinding}. */
export interface BrowserSyncWorkspaceSource {
	/** Stable workspace id. */
	workspaceId: string;
	/** Object that carries this replica's file changes. */
	objectId: string;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical decimal access-policy revision. */
	policyRevision: string;
	/** Object keys by object id; a key must exist for `objectId`. */
	objectKeys: ReadonlyMap<string, Uint8Array>;
	/** Per-object bindings by object id. */
	objects?: ReadonlyMap<string, BrowserSyncObjectBinding>;
	/** Pinned Ed25519 public keys by signing device id. */
	pinnedSigners: ReadonlyMap<string, Uint8Array>;
	/** The workspace replica. In the hosted app it lives in the workspace worker. */
	workspace: WorkspaceStorageLike;
	/** Account origin. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Injected transport. Defaults to a same-origin `fetch` wrapper. */
	fetch?: FetchLike;
	/** Durable state store. Defaults to {@link openBrowserSyncStateStore}. */
	state?: SyncStateStore;
	/** Clock used to timestamp conflicts. */
	now?: () => number;
}

/**
 * Bridge a workspace replica onto the engine's storage boundary while
 * preserving the engine's unguarded write and delete calls.
 *
 * The engine's bundled `createWorkspaceStorageAdapter` maps a missing
 * `expectedRevision` (an unguarded force-apply or force-delete used by
 * `resolveConflict`) to `null`, which `BrowserWorkspaceStorage` interprets as
 * "the path must be absent" and rejects. This adapter resolves the current
 * revision itself for those unguarded calls, so a user-chosen remote conflict
 * resolution can actually overwrite or delete local bytes, while every guarded
 * call still passes its expected revision straight through.
 */
export function createWorkspaceSyncStorage(
	workspace: WorkspaceStorageLike,
): BrowserSyncStorage {
	return {
		async read(path) {
			const file = await workspace.read(path);
			return file === null
				? null
				: { bytes: file.bytes, revision: file.revision };
		},
		async write({ path, bytes, expectedRevision }) {
			let expected: string | null;
			if (expectedRevision === undefined) {
				const current = await workspace.read(path);
				expected = current?.revision ?? null;
			} else {
				expected = expectedRevision;
			}
			const result = await workspace.write({
				path,
				bytes,
				expectedRevision: expected,
			});
			const revision = (result as { revision?: unknown } | null | undefined)
				?.revision;
			if (typeof revision !== 'string') {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.RequestFailed,
					'The workspace storage adapter did not return a revision',
				);
			}
			return { revision };
		},
		async move({ from, to, expectedRevision }) {
			await workspace.move({
				from,
				to,
				expectedRevision: requireOperationRevision('move', expectedRevision),
			});
		},
		async delete({ path, expectedRevision }) {
			if (expectedRevision === undefined) {
				const current = await workspace.read(path);
				if (current === null) return;
				await workspace.delete({ path, expectedRevision: current.revision });
				return;
			}
			await workspace.delete({ path, expectedRevision });
		},
		async list() {
			const rebuilt = await workspace.rebuild();
			const paths = new Set<string>();
			for (const file of rebuilt.files ?? []) {
				if (typeof file?.path === 'string') paths.add(file.path);
			}
			for (const managed of rebuilt.managed ?? []) {
				const managedPath = managed?.relativePath ?? managed?.path;
				if (typeof managedPath === 'string') paths.add(managedPath);
			}
			return [...paths].sort();
		},
	};
}

/**
 * Compose the concrete engine boundaries for one browser workspace.
 *
 * This uses {@link createWorkspaceSyncStorage} over the workspace replica, an
 * engine file-system state store, and a `BrowserSyncTransport`-backed remote.
 * It returns `null` when no durable state store can be opened (no OPFS), so the
 * caller can report an unsupported state instead of syncing against volatile
 * state.
 *
 * The hosted app supplies `source.workspace` through
 * {@link createWorkerWorkspaceStorage} over the workspace worker's raw file
 * operations.
 */
export async function createBrowserSyncWorkspaceBinding(
	source: BrowserSyncWorkspaceSource,
): Promise<BrowserSyncWorkspaceBinding | null> {
	const state =
		source.state ?? (await openBrowserSyncStateStore(source.workspaceId));
	if (!state) return null;
	return {
		workspaceId: source.workspaceId,
		objectId: source.objectId,
		epoch: source.epoch,
		policyRevision: source.policyRevision,
		objectKeys: source.objectKeys,
		...(source.objects === undefined ? {} : { objects: source.objects }),
		pinnedSigners: source.pinnedSigners,
		storage: createWorkspaceSyncStorage(source.workspace),
		state,
		remote: createBrowserSyncRemote({
			origin: source.origin,
			token: source.token,
			workspaceId: source.workspaceId,
			...(source.fetch === undefined ? {} : { fetch: source.fetch }),
		}),
		attachments: {
			origin: source.origin,
			token: source.token,
			fetch: source.fetch ?? createSameOriginFetch(source.origin),
		},
		...(source.now === undefined ? {} : { now: source.now }),
	};
}

interface OpfsStorageManager {
	getDirectory?: () => Promise<FileSystemDirectoryHandle>;
}

function opfsStorage(): OpfsStorageManager | null {
	const navigatorValue = globalThis.navigator as
		(Navigator & { storage?: OpfsStorageManager }) | undefined;
	const storage = navigatorValue?.storage;
	if (!storage || typeof storage.getDirectory !== 'function') return null;
	return storage;
}

async function openOpfsDirectory(
	path: string,
): Promise<FileSystemDirectoryHandle | null> {
	const storage = opfsStorage();
	if (!storage?.getDirectory) return null;
	let directory: FileSystemDirectoryHandle;
	try {
		directory = await storage.getDirectory();
	} catch {
		return null;
	}
	for (const part of path.split('/')) {
		if (part.length === 0) continue;
		directory = await directory.getDirectoryHandle(part, { create: true });
	}
	return directory;
}

function parseStoredBundle(text: string): WrappedKeyBundle {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser key bundle was not valid JSON',
		);
	}
	if (!value || typeof value !== 'object' || Array.isArray(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser key bundle had an unexpected shape',
		);
	}
	const record = value as Record<string, unknown>;
	const strings = [
		'id',
		'deviceId',
		'kdf',
		'salt',
		'nonce',
		'ciphertext',
		'signingPublic',
		'recipientPublic',
	];
	const valid =
		record.version === 1 &&
		strings.every((field) => typeof record[field] === 'string') &&
		typeof record.iterations === 'number';
	if (!valid) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser key bundle was missing required fields',
		);
	}
	return record as unknown as WrappedKeyBundle;
}

function requireBundleId(id: string): string {
	if (!isIdentifier(id)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The browser key bundle id was malformed',
		);
	}
	return id;
}

/**
 * Open the OPFS-backed wrapped-bundle store, or `null` when OPFS is absent.
 * Only wrapped ciphertext ever crosses this boundary.
 */
export async function openBrowserSyncKeyStore(): Promise<KeyStore | null> {
	const directory = await openOpfsDirectory(`${ADAPTER_DIRECTORY}/keys`);
	if (!directory) return null;
	return {
		async read(id) {
			const file = await readOpfsFile(directory, `${requireBundleId(id)}.json`);
			if (!file) return undefined;
			return parseStoredBundle(await file.text());
		},
		async write(id, bundle) {
			const handle = await directory.getFileHandle(
				`${requireBundleId(id)}.json`,
				{ create: true },
			);
			const writable = await handle.createWritable();
			try {
				await writable.write(JSON.stringify(bundle));
				await writable.close();
			} catch (error) {
				await writable.abort().catch(() => {});
				throw error;
			}
		},
		async delete(id) {
			try {
				await directory.removeEntry(`${requireBundleId(id)}.json`);
			} catch (error) {
				if (!isNotFoundError(error)) throw error;
			}
		},
	};
}

async function readOpfsFile(
	directory: FileSystemDirectoryHandle,
	name: string,
): Promise<File | null> {
	try {
		const handle = await directory.getFileHandle(name);
		return await handle.getFile();
	} catch (error) {
		if (isNotFoundError(error)) return null;
		throw error;
	}
}

function isNotFoundError(error: unknown): boolean {
	return error instanceof DOMException && error.name === 'NotFoundError';
}

/**
 * Open the durable engine state store for one workspace. The file lives under
 * the adapter-owned directory, never inside canonical workspace files, so it is
 * never enumerated, exported, or synchronized. Returns `null` without OPFS.
 */
export async function openBrowserSyncStateStore(
	workspaceId: string,
): Promise<SyncStateStore | null> {
	const directory = await openOpfsDirectory(`${ADAPTER_DIRECTORY}/state`);
	if (!directory) return null;
	return createFileSystemSyncStateStore(
		new OpfsFileSystem(directory),
		`${requireBundleId(workspaceId)}.json`,
	);
}

/**
 * Open the durable binding store for one local browser workspace.
 *
 * The record lives under the adapter-owned directory, outside canonical
 * workspace files and outside the sync state file, so it is never enumerated,
 * exported, or synchronized. It stores only the workspace/object identity, the
 * access-policy revision, pinned signer keys, and self-wrapped object keys.
 * Returns `null` without OPFS.
 */
export async function openBrowserSyncBindingStore(
	localWorkspaceId: string,
): Promise<BrowserSyncBindingStore | null> {
	const directory = await openOpfsDirectory(`${ADAPTER_DIRECTORY}/bindings`);
	if (!directory) return null;
	const name = `${requireBundleId(localWorkspaceId)}.json`;
	return {
		async read() {
			const file = await readOpfsFile(directory, name);
			if (!file) return null;
			return parseBindingRecord(await file.text());
		},
		async write(record) {
			const handle = await directory.getFileHandle(name, { create: true });
			const writable = await handle.createWritable();
			try {
				await writable.write(JSON.stringify(record));
				await writable.close();
			} catch (error) {
				await writable.abort().catch(() => {});
				throw error;
			}
		},
		async remove() {
			try {
				await directory.removeEntry(name);
				return true;
			} catch (error) {
				if (isNotFoundError(error)) return false;
				throw error;
			}
		},
	};
}

/**
 * Adapt raw worker file operations to the engine's `WorkspaceStorageLike`.
 *
 * Only syncable paths are enumerated: the canonical sync schema rejects the
 * reserved `.noura`, `.git`, `node_modules`, and `target` roots, so the engine
 * must never see them in `list()`.
 */
export function createWorkerWorkspaceStorage(
	files: BrowserWorkspaceFiles,
): WorkspaceStorageLike {
	return {
		read: (path) => files.read(path),
		write: (input) =>
			files.write({
				path: input.path,
				bytes: input.bytes,
				expectedRevision: input.expectedRevision ?? null,
			}),
		move: (input) =>
			files.move({
				from: input.from,
				to: input.to,
				expectedRevision: requireOperationRevision(
					'move',
					input.expectedRevision,
				),
				expectedDestinationRevision: null,
			}),
		delete: (input) =>
			files.delete({
				path: input.path,
				expectedRevision: requireOperationRevision(
					'delete',
					input.expectedRevision,
				),
			}),
		async rebuild() {
			const paths = (await files.list()).filter(isSyncablePath);
			return { files: paths.map((path) => ({ path })), managed: [] };
		},
	};
}

function requireOperationRevision(
	action: 'move' | 'delete',
	revision: string | null | undefined,
): string {
	if (typeof revision !== 'string')
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			`A workspace ${action} requires the current revision`,
		);
	return revision;
}

function authorizationHeaders(token: string): Record<string, string> {
	return {
		authorization: `Bearer ${token}`,
		'content-type': 'application/json',
		accept: 'application/json',
	};
}

/** Create the remote workspace that a browser-only first device owns. */
async function createRemoteWorkspace(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
}): Promise<void> {
	const response = await input.fetch(
		new URL('/v1/workspaces', input.origin).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify({ id: input.workspaceId }),
		},
	);
	ensureResponseOk(response);
}

/** Create one remote sync object and return its key epoch. */
async function createRemoteObject(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	objectId: string;
}): Promise<number> {
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/objects`,
			input.origin,
		).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify({ id: input.objectId }),
		},
	);
	ensureResponseOk(response);
	const data = (await readJson(response)) as { epoch?: unknown } | null;
	const epoch = data?.epoch;
	if (typeof epoch !== 'number' || !Number.isSafeInteger(epoch) || epoch < 1) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'The sync server returned an invalid object epoch',
		);
	}
	return epoch;
}

/** Upload the signed version-1 access policy for the new workspace. */
async function putRemoteAccessPolicy(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	policy: AccessPolicy;
}): Promise<void> {
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/access`,
			input.origin,
		).toString(),
		{
			method: 'PUT',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify(input.policy),
		},
	);
	if (response.ok) return;
	let serverCode: string | undefined;
	try {
		const body = (await response.json()) as {
			error?: { code?: unknown };
		} | null;
		if (body && typeof body === 'object' && body.error) {
			const code = body.error.code;
			if (typeof code === 'string') serverCode = code;
		}
	} catch {
		// A non-JSON error body carries no structured code; fall through.
	}
	const options = {
		status: response.status,
		...(serverCode === undefined ? {} : { cause: { serverCode } }),
	};
	if (response.status === 401 || response.status === 403) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.Unauthorized,
			serverCode ?? 'request was not authorized',
			options,
		);
	}
	throw new BrowserSyncError(
		BrowserSyncErrorCode.RequestFailed,
		serverCode ?? `request failed with status ${response.status}`,
		options,
	);
}

/** Convert a self-wrapped key envelope into the persisted binding shape. */
function toBoundKey(
	envelope: WebKeyEnvelope,
	deviceId: string,
): BrowserSyncBoundKey {
	return {
		deviceId,
		wrappedKey: envelope.wrapped_key,
		signature: envelope.signature,
		construction: 'web',
		recipientPublicKey: envelope.recipient_public_key,
		ephemeralPublicKey: envelope.ephemeral_public_key,
		salt: envelope.salt,
		nonce: envelope.nonce,
	};
}

/** Convert a wrapped key envelope into the signed access-policy shape. */
function toPolicyEnvelope(
	envelope: WebKeyEnvelope,
	deviceId: string,
): AccessPolicyEnvelope {
	return {
		deviceId,
		wrappedKey: envelope.wrapped_key,
		signature: envelope.signature,
		construction: 'web',
		recipientPublicKey: envelope.recipient_public_key,
		ephemeralPublicKey: envelope.ephemeral_public_key,
		salt: envelope.salt,
		nonce: envelope.nonce,
	};
}

/**
 * True when a policy upload was rejected because the workspace access revision
 * moved under us. The server reports this as `sync.policy_revision_changed`
 * (409); the caller should re-read access-state once and retry.
 */
function isPolicyRevisionChanged(error: unknown): boolean {
	return (
		error instanceof BrowserSyncError &&
		error.status === 409 &&
		(error.cause as { serverCode?: unknown } | undefined)?.serverCode ===
			'sync.policy_revision_changed'
	);
}

/** Convert a persisted binding entry back into a verifiable key envelope. */
function toWebEnvelope(
	record: BrowserSyncBindingRecord,
	objectId: string,
	bound: BrowserSyncBoundObject,
): WebKeyEnvelope {
	return {
		workspace_id: record.workspaceId,
		object_id: objectId,
		epoch: bound.epoch,
		signing_device: bound.key.deviceId,
		device_id: bound.key.deviceId,
		recipient_public_key: bound.key.recipientPublicKey,
		ephemeral_public_key: bound.key.ephemeralPublicKey,
		salt: bound.key.salt,
		nonce: bound.key.nonce,
		wrapped_key: bound.key.wrappedKey,
		signature: bound.key.signature,
	};
}

/** Verify and unwrap one persisted object key with the unlocked identity. */
async function unwrapBoundKey(
	record: BrowserSyncBindingRecord,
	objectId: string,
	bound: BrowserSyncBoundObject,
	identity: DeviceIdentity,
): Promise<Uint8Array> {
	return unwrapKey(
		toWebEnvelope(record, objectId, bound),
		identity.x25519Secret,
		identity.signingPublic,
	);
}

/** Public fields extracted from a native `noura.sync.recovery` kit file. */
export interface ParsedNativeRecoveryKit {
	/** Signed public recovery object; the library validates its internals. */
	recovery: NativeRecoveryObject;
	/** Embedded age recovery identity when the kit records one, else `null`. */
	recoveryIdentity: string | null;
	/** Base64 Ed25519 recovery signer the recovery object self-describes. */
	recoverySignerPublic: string | null;
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
 * The Ed25519 recovery signer a native recovery object self-describes.
 *
 * The recovery object's own (signed) configuration pins the public key of the
 * device that authored it. Returning it here lets the importer verify the
 * recovery signature, but only when the caller has not pinned a different key.
 */
export function nativeRecoverySignerPublic(
	recovery: NativeRecoveryObject,
): string | null {
	const config = recovery.config as unknown;
	if (!config || typeof config !== 'object' || Array.isArray(config))
		return null;
	const record = config as Record<string, unknown>;
	const deviceId = record.deviceId;
	const trusted = record.trustedDevices;
	if (typeof deviceId !== 'string') return null;
	if (!trusted || typeof trusted !== 'object' || Array.isArray(trusted))
		return null;
	const pin = (trusted as Record<string, unknown>)[deviceId];
	return typeof pin === 'string' ? pin : null;
}

/**
 * Parse a native `noura.sync.recovery` kit file.
 *
 * This validates the file wrapper and extracts the embedded recovery identity
 * and self-described recovery signer. The signed recovery object's own fields
 * and signatures are validated by `importNativeRecoveryKit`; a file that is not
 * a native recovery kit, including a browser `noura.browser-recovery-kit`, is
 * rejected rather than guessed.
 */
export function parseNativeRecoveryKit(file: unknown): ParsedNativeRecoveryKit {
	if (!file || typeof file !== 'object' || Array.isArray(file)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit was not an object.',
		);
	}
	const kit = file as Record<string, unknown>;
	const format = kit.format ?? kit.domain;
	if (format !== NATIVE_RECOVERY_DOMAIN) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The file was not a native Noura recovery kit.',
		);
	}
	const recovery = kit.recovery;
	if (!recovery || typeof recovery !== 'object' || Array.isArray(recovery)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit was missing its recovery object.',
		);
	}
	const rawIdentity = kit.recovery_identity;
	if (
		rawIdentity !== undefined &&
		(typeof rawIdentity !== 'string' || rawIdentity.length === 0)
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit had a malformed recovery identity.',
		);
	}
	return {
		recovery: recovery as NativeRecoveryObject,
		recoveryIdentity: typeof rawIdentity === 'string' ? rawIdentity : null,
		recoverySignerPublic: nativeRecoverySignerPublic(
			recovery as NativeRecoveryObject,
		),
	};
}

/**
 * Extract the recovery identity embedded in a parsed native recovery kit, or
 * `null` when the kit does not record one. The caller must then require the user
 * to paste it. A malformed or non-native file throws instead of being ignored.
 */
export function extractEmbeddedRecoveryIdentity(file: unknown): string | null {
	return parseNativeRecoveryKit(file).recoveryIdentity;
}

/** Resolve the recovery signer, rejecting an inconsistent self-description. */
function resolveNativeRecoverySigner(
	parsed: ParsedNativeRecoveryKit,
	pinned: string | undefined,
): { ok: true; value: Uint8Array } | { ok: false; message: string } {
	const selfDescribed = parsed.recoverySignerPublic;
	if (
		pinned !== undefined &&
		selfDescribed !== null &&
		pinned !== selfDescribed
	) {
		return {
			ok: false,
			message:
				'The supplied recovery signer does not match the signer the kit self-describes; refusing to trust either.',
		};
	}
	const chosen = pinned ?? selfDescribed;
	if (chosen === undefined || chosen === null) {
		return {
			ok: false,
			message:
				'This kit does not record its recovery signer, so the signer public key must be supplied.',
		};
	}
	try {
		return { ok: true, value: decodeBase64(chosen, 32) };
	} catch {
		return {
			ok: false,
			message:
				'The recovery signer public key was not a 32-byte base64 Ed25519 key.',
		};
	}
}

/**
 * Build the binding's per-object metadata from a native recovery object.
 *
 * A native kit does not carry the browser replica's canonical paths, so each
 * recovered object is anchored to its native object id. Object ids are
 * validated as identifiers (never canonical file paths) and the persisted
 * binding marks every recovered object `unmapped`, so a crafted kit cannot make
 * a local file or attachment be sealed under a recovered key. The binding holds
 * the re-wrapped key until the replica is reconciled.
 */
function nativeRecoveryObjects(recovery: NativeRecoveryObject): {
	objectId: string;
	objects: RecoverNativeKeysToBrowserBindingInput['objects'];
} | null {
	const envelopes = Array.isArray(recovery.envelopes) ? recovery.envelopes : [];
	const highest = new Map<string, number>();
	for (const raw of envelopes) {
		const envelope = raw as NativeRecoveryEnvelope;
		if (
			!envelope ||
			!isIdentifier(envelope.objectId) ||
			!Number.isSafeInteger(envelope.epoch) ||
			envelope.epoch < 1
		)
			continue;
		const current = highest.get(envelope.objectId) ?? 0;
		if (envelope.epoch > current)
			highest.set(envelope.objectId, envelope.epoch);
	}
	const objects: RecoverNativeKeysToBrowserBindingInput['objects'] = {};
	let primary: string | null = null;
	for (const [objectId, epoch] of highest) {
		if (primary === null) primary = objectId;
		objects[objectId] = { path: objectId, epoch, policyRevision: '1' };
	}
	return primary === null ? null : { objectId: primary, objects };
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		const identity = this.#identity;
		if (!identity) {
			return failure(
				'locked',
				'Unlock this browser device before enabling sync.',
			);
		}
		if (!identity.token) {
			return failure(
				'not_configured',
				'This browser device is not enrolled with the sync server yet.',
			);
		}
		const workspaceFiles = input.workspaceFiles ?? this.#workspaceFiles;
		if (!workspaceFiles) {
			return failure(
				'not_configured',
				'No browser workspace is open, so there is nothing to bind to sync.',
			);
		}
		if (!this.#origin) {
			return failure(
				'not_configured',
				'This page is not served from a Noura sync origin, so sync cannot be enabled.',
			);
		}
		try {
			const bindingStore = await this.#resolveBindingStore(input.workspaceId);
			if (!bindingStore) {
				return failure(
					'unavailable',
					'This browser cannot persist a durable sync binding (OPFS is unavailable).',
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
						'This browser cannot open the durable sync state for this workspace.',
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
					'No durable sync binding exists for this browser workspace yet.',
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		if (!passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter a passphrase to protect the recovery kit.',
			);
		}
		if (!this.#identity || !this.#bundle) {
			return failure(
				'locked',
				'Unlock this browser device before exporting a recovery kit.',
			);
		}
		try {
			const record = this.#record ?? (await this.#loadRecord());
			if (!record) {
				return failure(
					'not_configured',
					'No browser sync binding is loaded, so there is nothing to include in a recovery kit.',
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		if (!passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter the recovery kit passphrase.',
			);
		}
		if (!isIdentifier(localWorkspaceId)) {
			return failure(
				'not_configured',
				'This browser workspace has an invalid local id.',
			);
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
			return failure(
				'not_configured',
				'The recovery kit binding had a malformed workspace id.',
			);
		}
		const store = await this.#resolveBindingStore(localWorkspaceId);
		if (!store) {
			return failure(
				'unavailable',
				'This browser cannot persist a durable sync binding (OPFS is unavailable).',
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		const identity = this.#identity;
		if (!this.#bundle || !identity) {
			return failure(
				'locked',
				'Unlock this browser device before importing a native recovery kit. No browser device was created.',
			);
		}
		if (!identity.token) {
			return failure(
				'not_configured',
				'This browser device is not enrolled with the sync server yet.',
			);
		}
		const localWorkspaceId = options.localWorkspaceId ?? this.#localWorkspaceId;
		if (!localWorkspaceId || !isIdentifier(localWorkspaceId)) {
			return failure(
				'not_configured',
				'Open a browser workspace before importing a native recovery kit.',
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
			return failure(
				'custody_failed',
				'Enter the native recovery identity, or paste the one the kit embeds.',
			);
		}
		if (
			parsed.recoveryIdentity !== null &&
			parsed.recoveryIdentity !== recoveryIdentity
		) {
			return failure(
				'custody_failed',
				'The pasted recovery identity does not match the identity embedded in this kit.',
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
				'The native recovery kit carried no object keys to recover.',
			);
		}
		const store = await this.#resolveBindingStore(localWorkspaceId);
		if (!store) {
			return failure(
				'unavailable',
				'This browser cannot persist a durable sync binding (OPFS is unavailable).',
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
				'This browser workspace is already bound to a different sync workspace; clear the existing binding before importing a recovery kit.',
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
				'This browser workspace has no managed note, task, or project files, so there is nothing to synchronize yet.',
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
					'The active device set did not include this browser device, so no object key could be wrapped to it.',
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
				'This browser cannot open durable sync state (OPFS is unavailable).',
			);
		}
		const primary = objects[primaryId];
		if (!primary) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidOperation,
				'This browser workspace had no managed object to bind.',
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
				'This browser cannot open durable sync state (OPFS is unavailable).',
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		if (!input.passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter a passphrase to protect this browser device.',
			);
		}
		if (!this.#origin) {
			return failure(
				'not_configured',
				'This page is not served from a Noura sync origin, so a browser device cannot enroll.',
			);
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		if (!input.passphrase) {
			return failure(
				'invalid_passphrase',
				'Enter your passphrase to unlock this browser device.',
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
			return failure(
				'not_configured',
				'No browser device is set up on this browser. Enroll this browser first.',
			);
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		}
		const identity = this.#identity;
		if (!identity) {
			return failure(
				'locked',
				'Unlock this browser device before binding a synchronized workspace.',
			);
		}
		const workspaceFiles = input.workspaceFiles ?? this.#workspaceFiles;
		if (!workspaceFiles) {
			return failure(
				'not_configured',
				'No browser workspace is open, so there is nothing to bind to sync.',
			);
		}
		this.#localWorkspaceId = input.workspaceId;
		this.#workspaceFiles = workspaceFiles;
		try {
			const record = await this.#loadRecord(input.workspaceId);
			if (!record) {
				return failure(
					'not_configured',
					'No durable sync binding exists for this browser workspace yet.',
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
					'This browser cannot open the durable sync state for this workspace.',
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
					'The active device set did not include this browser device, so no object key could be wrapped to it.',
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
					'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			};
		}
		const identity = this.#identity;
		if (!identity) {
			return {
				status: 'locked',
				message: 'Unlock this browser device before syncing.',
			};
		}
		if (!identity.token) {
			return {
				status: 'not_configured',
				message:
					'This browser device is not enrolled with the sync server yet.',
			};
		}
		let binding = this.#binding;
		if (!binding) {
			binding = await this.#ensureBinding(input.workspaceId).catch(() => null);
		}
		if (!binding) {
			return {
				status: 'not_configured',
				message:
					'No synchronized browser workspace is configured on this tab yet. Nothing was uploaded or changed.',
			};
		}
		if (
			binding.objectKeys.size === 0 ||
			!binding.objectKeys.has(binding.objectId)
		) {
			return {
				status: 'not_configured',
				message:
					'No workspace object key map is available for this browser workspace, so no encrypted operation can be sealed.',
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
						'This browser device or workspace access was revoked. Lock the browser and re-enroll when access is restored.',
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
					'Another device on this account cannot receive browser key envelopes. Create the workspace from that device and have it deliver keys to this browser.',
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
			return failure('locked', 'Unlock this browser device to list devices.');
		}
		if (!identity.token) {
			return failure(
				'not_configured',
				'This browser device is not enrolled with the sync server yet.',
			);
		}
		const record = await this.#loadRecord(input.workspaceId).catch(() => null);
		const workspaceId = this.#binding?.workspaceId ?? record?.workspaceId;
		if (!workspaceId) {
			return failure(
				'not_configured',
				'No synchronized browser workspace is configured yet.',
			);
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
			return failure('device_not_found', 'A device id is required.');
		}
		const identity = this.#identity;
		if (!identity) {
			return failure(
				'locked',
				'Unlock this browser device to approve a device.',
			);
		}
		const record = await this.#loadRecord().catch(() => null);
		const store = record
			? await this.#resolveBindingStore(record.localWorkspaceId)
			: null;
		if (!record || !store) {
			return failure(
				'not_configured',
				'No durable sync binding exists for this browser workspace yet.',
			);
		}
		const listed = await this.listWorkspaceDevices();
		if (!listed.ok) return failure(listed.code, listed.message);
		const device = listed.value.find((entry) => entry.deviceId === deviceId);
		if (!device) {
			return failure(
				'device_not_found',
				'The workspace access state did not include that device.',
			);
		}
		if (device.fingerprint !== fingerprint) {
			return failure(
				'fingerprint_mismatch',
				'The supplied fingerprint did not match the device card; nothing was approved.',
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
			return failure(
				'not_configured',
				'No durable sync binding exists for this browser workspace yet.',
			);
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
			return failure(
				'locked',
				'Unlock this browser device before resolving a conflict.',
			);
		}
		let binding = this.#binding;
		if (!binding) {
			binding = await this.#ensureBinding().catch(() => null);
		}
		if (!binding) {
			return failure(
				'not_configured',
				'No synchronized browser workspace is configured yet.',
			);
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
			return failure(
				'locked',
				'Unlock this browser device to list attachments.',
			);
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure(
				'not_configured',
				'No synchronized browser workspace is configured yet.',
			);
		const target = await this.#resolveAttachmentObject(input);
		if (!target)
			return failure(
				'object_not_found',
				'This note is not synchronized with an object that can own attachments.',
			);
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
			return failure(
				'locked',
				'Unlock this browser device to download an attachment.',
			);
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure(
				'not_configured',
				'No synchronized browser workspace is configured yet.',
			);
		const target = await this.#resolveAttachmentObject(input);
		if (!target || !isAttachmentPathFor(target.objectId, input.path))
			return failure(
				'object_not_found',
				'That attachment does not belong to the open note.',
			);
		try {
			const file = await binding.storage.read(input.path);
			if (file === null)
				return failure(
					'attachment_unavailable',
					'That attachment is not stored in this browser.',
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
				'This browser cannot store wrapped device keys securely (OPFS is unavailable).',
			);
		const identity = this.#identity;
		if (!identity)
			return failure('locked', 'Unlock this browser device to attach a file.');
		if (!identity.token)
			return failure(
				'not_configured',
				'This browser device is not enrolled with the sync server yet.',
			);
		const binding =
			this.#binding ?? (await this.#ensureBinding().catch(() => null));
		if (!binding)
			return failure(
				'not_configured',
				'No synchronized browser workspace is configured yet.',
			);
		if (!binding.attachments)
			return failure(
				'not_configured',
				'No sync transport is configured, so an attachment cannot be uploaded.',
			);
		const target = await this.#resolveAttachmentObject(input);
		if (!target)
			return failure(
				'object_not_found',
				'This note is not synchronized with an object that can own attachments. Sync the note first.',
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
				return failure(
					'locked',
					'This browser device or workspace access was revoked.',
				);
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

let sharedController: Promise<BrowserSyncController> | undefined;

/**
 * The app-wide controller. The account page and the workspace shell share one
 * in-memory unlock state for this tab. Tests should call
 * {@link createBrowserSyncController} directly.
 */
export function getBrowserSyncController(): Promise<BrowserSyncController> {
	sharedController ??= createBrowserSyncController();
	return sharedController;
}

/** Drop the shared controller so the next caller rebuilds it (tests, sign-out). */
export function resetBrowserSyncController(): void {
	sharedController = undefined;
}
