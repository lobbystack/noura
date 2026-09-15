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
 *   exists; without one it returns a typed `not_configured` result. Nothing is
 *   faked: the local replica is the workspace worker, reached through the raw
 *   canonical file operations, and no result is reported without an actual
 *   reconcile.
 * - The unwrapped object key lives only in tab memory. The binding stores a
 *   `noura.sync.key.web` envelope wrapped to this browser device, never a
 *   plaintext object key.
 * - The remote speaks the same bearer-token transport as the native client, and
 *   only same-origin requests are permitted.
 */

import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	BrowserSyncTransport,
	createDeviceIdentity,
	createFileChangeCodec,
	decodeBase64,
	encodeBase64,
	ensureResponseOk,
	enrollBrowserDevice,
	isIdentifier,
	randomBytes,
	randomIdentifier,
	readJson,
	requestDeviceChallenge,
	sealIdentity,
	signAccessPolicy,
	unlockDeviceIdentity,
	unwrapKey,
	wrapKey,
	type AccessPolicy,
	type AccessPolicyEnvelope,
	type DeviceIdentity,
	type FetchLike,
	type KeyStore,
	type WebKeyEnvelope,
	type WrappedKeyBundle,
} from '@noura/browser-sync';
import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
import {
	BrowserSyncEngine,
	BrowserSyncEngineError,
	BrowserSyncEngineErrorCode,
	createFileSystemSyncStateStore,
	createWorkspaceStorageAdapter,
	type BrowserSyncEngineOptions,
	type BrowserSyncRemote,
	type BrowserSyncStorage,
	type FileChangeCodec as EngineFileChangeCodec,
	type ReconcileResult,
	type SyncStateStore,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import { OpfsFileSystem } from '@noura/browser-storage';

/** Directory outside canonical workspace files that holds adapter-owned state. */
const ADAPTER_DIRECTORY = '.noura-adapter/browser-sync';

/** Default stable key for this browser's wrapped device bundle. */
export const DEFAULT_BROWSER_SYNC_BUNDLE_ID = 'browser-device';

/** Anchor recorded for the workspace-level sync object; informational only. */
const BINDING_PATH_ANCHOR = '.noura/workspace.yaml';

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
 * Wrapped object-key material persisted in the binding record.
 *
 * The object key itself is never written to origin storage in plaintext. This is
 * the `noura.sync.key.web` envelope bound to the browser's own device, so it can
 * be unwrapped only after the device bundle is unlocked.
 */
export interface BrowserSyncBoundKey {
	deviceId: string;
	wrappedKey: string;
	signature: string;
	construction: 'web';
	recipientPublicKey: string;
	ephemeralPublicKey: string;
	salt: string;
	nonce: string;
}

/** One remote sync object bound to this browser workspace. */
export interface BrowserSyncBoundObject {
	/** Informational canonical-path anchor for the object. */
	path: string;
	/** Positive safe-integer object key epoch. */
	epoch: number;
	/** Canonical access-policy revision the object was bound at. */
	policyRevision: string;
	/** Self-wrapped object key; never plaintext. */
	key: BrowserSyncBoundKey;
}

/** Durable browser sync binding record. Contains no unwrapped key material. */
export interface BrowserSyncBindingRecord {
	version: 1;
	/** Stable ID of the local browser workspace this binding belongs to. */
	localWorkspaceId: string;
	/** Remote workspace identifier. */
	workspaceId: string;
	/** Access-policy revision last persisted. */
	revision: string;
	/** Primary sync object carrying this workspace's file changes. */
	objectId: string;
	/** Object bindings by object id. */
	objects: Record<string, BrowserSyncBoundObject>;
	/** Pinned signer public keys by device id, base64. */
	pinnedSigners: Record<string, string>;
}

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

function isBindingRecord(value: unknown): value is BrowserSyncBindingRecord {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	const record = value as Record<string, unknown>;
	if (
		record.version !== 1 ||
		typeof record.localWorkspaceId !== 'string' ||
		typeof record.workspaceId !== 'string' ||
		typeof record.revision !== 'string' ||
		typeof record.objectId !== 'string' ||
		!record.objects ||
		typeof record.objects !== 'object' ||
		Array.isArray(record.objects) ||
		!record.pinnedSigners ||
		typeof record.pinnedSigners !== 'object' ||
		Array.isArray(record.pinnedSigners)
	)
		return false;
	const objects = record.objects as Record<string, unknown>;
	for (const bound of Object.values(objects)) {
		if (!bound || typeof bound !== 'object' || Array.isArray(bound))
			return false;
		const value = bound as Record<string, unknown>;
		if (
			typeof value.path !== 'string' ||
			!Number.isSafeInteger(value.epoch) ||
			(value.epoch as number) < 1 ||
			typeof value.policyRevision !== 'string' ||
			!value.key ||
			typeof value.key !== 'object' ||
			Array.isArray(value.key)
		)
			return false;
		const key = value.key as Record<string, unknown>;
		if (
			typeof key.deviceId !== 'string' ||
			typeof key.wrappedKey !== 'string' ||
			typeof key.signature !== 'string' ||
			key.construction !== 'web' ||
			typeof key.recipientPublicKey !== 'string' ||
			typeof key.ephemeralPublicKey !== 'string' ||
			typeof key.salt !== 'string' ||
			typeof key.nonce !== 'string'
		)
			return false;
	}
	if (
		!(record.objectId in objects) ||
		!Object.values(record.pinnedSigners).every(
			(value) => typeof value === 'string',
		)
	)
		return false;
	return true;
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
	if (!isBindingRecord(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser sync binding had an unexpected shape',
		);
	}
	return value;
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
	| 'sync_failed';

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
	  }
	| { status: 'unavailable'; message: string }
	| { status: 'locked'; message: string }
	| { status: 'not_configured'; message: string }
	| { status: 'revoked'; message: string }
	| { status: 'error'; message: string };

/** Read-only counters for the currently bound workspace replica. */
export interface BrowserSyncWorkspaceSummary {
	configured: boolean;
	workspaceId: string | null;
	cursor: string;
	pending: number;
	conflicts: number;
}

/**
 * The injectable boundaries one browser workspace replica needs. A host binds
 * these once `BrowserWorkspaceStorage` is reachable from the same thread as the
 * controller (today it is not, so the hosted UI leaves this unset).
 */
export interface BrowserSyncWorkspaceBinding {
	/** Stable workspace id. */
	workspaceId: string;
	/** Object that carries this replica's the file changes. */
	objectId: string;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical decimal access-policy revision. */
	policyRevision: string;
	/** Object keys by object id; a key must exist for `objectId`. */
	objectKeys: ReadonlyMap<string, Uint8Array>;
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
		return fetchImpl(input, init);
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
 * Seal and apply one reconcile pass over the injected boundaries.
 *
 * Local changes are snapshotted, sealed into the durable outbox, and flushed by
 * the engine before it pulls and applies remote operations. This performs
 * cryptography through `@noura/browser-sync`'s file-change codec but no network
 * I/O of its own.
 */
export async function runBrowserSyncReconcile(
	input: BrowserSyncReconcileInput,
): Promise<ReconcileResult> {
	const pinnedSigners = new Map<string, string>();
	for (const [deviceId, key] of input.pinnedSigners) {
		pinnedSigners.set(deviceId, encodeBase64(key));
	}
	const baseCodec = createFileChangeCodec({
		identity: input.identity,
		objectKeys: input.objectKeys,
		pinnedSigners,
	});
	const codec: EngineFileChangeCodec = {
		sealFileChange(change) {
			return baseCodec.sealFileChange({
				workspaceId: input.workspaceId,
				objectId: input.objectId,
				epoch: input.epoch,
				policyRevision: input.policyRevision,
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
			};
		},
	};
	const engineOptions: BrowserSyncEngineOptions = {
		storage: input.storage,
		remote: input.remote,
		codec,
		state: input.state,
		...(input.now === undefined ? {} : { now: input.now }),
		...(input.onRevoked === undefined ? {} : { onRevoked: input.onRevoked }),
	};
	const engine = new BrowserSyncEngine(engineOptions);
	const changes = await engine.snapshotLocalChanges();
	for (const change of changes) await engine.enqueueFileChange(change);
	return engine.reconcile();
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
 * Compose the concrete engine boundaries for one browser workspace.
 *
 * This uses `createWorkspaceStorageAdapter` over the workspace replica, an
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
		pinnedSigners: source.pinnedSigners,
		storage: createWorkspaceStorageAdapter(source.workspace),
		state,
		remote: createBrowserSyncRemote({
			origin: source.origin,
			token: source.token,
			workspaceId: source.workspaceId,
			...(source.fetch === undefined ? {} : { fetch: source.fetch }),
		}),
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
	ensureResponseOk(response);
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
	#stateStore: SyncStateStore | null | undefined;

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
			const bindingStore =
				this.#bindingStore !== undefined
					? this.#bindingStore
					: await openBrowserSyncBindingStore(input.workspaceId);
			if (!bindingStore) {
				return failure(
					'unavailable',
					'This browser cannot persist a durable sync binding (OPFS is unavailable).',
				);
			}
			const existing = await bindingStore.read();
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
				this.#workspaceFiles = workspaceFiles;
				return ok(await this.workspaceSummary());
			}
			if (input.create === false) {
				return failure(
					'not_configured',
					'No durable sync binding exists for this browser workspace yet.',
				);
			}
			const binding = await this.#bootstrapBinding(
				input.workspaceId,
				identity,
				workspaceFiles,
				bindingStore,
			);
			this.#binding = binding;
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

	async #bootstrapBinding(
		localWorkspaceId: string,
		identity: DeviceIdentity,
		workspaceFiles: BrowserWorkspaceFiles,
		bindingStore: BrowserSyncBindingStore,
	): Promise<BrowserSyncWorkspaceBinding> {
		const workspaceId = `ws_${randomIdentifier()}`;
		const objectId = `obj_${randomIdentifier()}`;
		const objectKey = randomBytes(32);
		const challenge = await requestDeviceChallenge(this.#fetch);
		await createRemoteWorkspace({
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			workspaceId,
		});
		const epoch = await createRemoteObject({
			origin: this.#origin,
			token: identity.token,
			fetch: this.#fetch,
			workspaceId,
			objectId,
		});
		const envelope = await wrapKey({
			workspace_id: workspaceId,
			object_id: objectId,
			epoch,
			signing_device: identity.deviceId,
			device_id: identity.deviceId,
			signing_secret: encodeBase64(identity.signingSeed),
			recipient_public: encodeBase64(identity.x25519Public),
			object_key: encodeBase64(objectKey),
			ephemeral_secret: encodeBase64(randomBytes(32)),
			salt: encodeBase64(randomBytes(32)),
			nonce: encodeBase64(randomBytes(12)),
		});
		const policyEnvelope: AccessPolicyEnvelope = toBoundKey(
			envelope,
			identity.deviceId,
		);
		const draft: Omit<AccessPolicy, 'signature'> = {
			version: 1,
			workspaceId,
			revision: '1',
			previousPolicyDigest: null,
			deviceId: identity.deviceId,
			members: [{ accountId: challenge.accountId, role: 'owner' }],
			objects: [
				{
					objectId,
					epoch,
					grants: [],
					envelopes: [policyEnvelope],
				},
			],
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
			objectId,
			objects: {
				[objectId]: {
					path: BINDING_PATH_ANCHOR,
					epoch,
					policyRevision: policy.revision,
					key: toBoundKey(envelope, identity.deviceId),
				},
			},
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
		const binding = await createBrowserSyncWorkspaceBinding({
			workspaceId,
			objectId,
			epoch,
			policyRevision: policy.revision,
			objectKeys: new Map([[objectId, objectKey]]),
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
		return binding;
	}

	async #buildBinding(
		record: BrowserSyncBindingRecord,
		identity: DeviceIdentity,
		workspaceFiles: BrowserWorkspaceFiles,
	): Promise<BrowserSyncWorkspaceBinding | null> {
		const objectKeys = new Map<string, Uint8Array>();
		for (const [objectId, bound] of Object.entries(record.objects)) {
			objectKeys.set(
				objectId,
				await unwrapBoundKey(record, objectId, bound, identity),
			);
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

	/** Durable cursor, pending outbox, and conflict counts for the bound workspace. */
	async workspaceSummary(): Promise<BrowserSyncWorkspaceSummary> {
		const binding = this.#binding;
		if (!binding) {
			return {
				configured: false,
				workspaceId: null,
				cursor: '0',
				pending: 0,
				conflicts: 0,
			};
		}
		try {
			const state = await binding.state.read();
			return {
				configured: true,
				workspaceId: binding.workspaceId,
				cursor: state.cursor,
				pending: state.outbox.length,
				conflicts: state.conflicts.length,
			};
		} catch {
			return {
				configured: false,
				workspaceId: binding.workspaceId,
				cursor: '0',
				pending: 0,
				conflicts: 0,
			};
		}
	}

	/**
	 * Reconcile the bound workspace once, or return a typed non-success outcome.
	 *
	 * This never fabricates a result: without an unlocked enrolled identity, an
	 * explicit object-key map, and the injectable replica boundaries it reports
	 * `locked`, `not_configured`, or `unavailable` and performs no I/O.
	 */
	async syncNow(): Promise<SyncNowOutcome> {
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
		const binding = this.#binding;
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
