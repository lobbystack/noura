/**
 * Browser replica reconciliation over injectable storage, transport, codec, and
 * durable state boundaries.
 *
 * The engine seals local file changes into a durable outbox before touching the
 * network, flushes the outbox in order, pulls encrypted operations from a stored
 * cursor, and applies the version-1 file-change rules with expected revisions.
 * A revision or absence mismatch becomes a {@link SyncConflict}; existing bytes
 * are never overwritten.
 *
 * The engine holds no keys and performs no cryptography. A codec or remote
 * error whose `code` is `"revoked"` (or
 * {@link BrowserSyncEngineErrorCode.Revoked}) transitions the engine to a locked
 * state, calls the injected `onRevoked` hook so the host can clear key material,
 * and refuses further reconciliation. Persisted remote replicas are left for the
 * caller to purge.
 *
 * The engine never logs workspace plaintext or key material.
 */
import type { EncryptedOperation, SyncPage } from '@noura/shared';
import { bytesEqual, isCursor } from '@noura/browser-sync';
import {
	BrowserSyncEngineError,
	BrowserSyncEngineErrorCode,
	isRevokedError,
} from './errors';
import { cloneSyncState, validateSyncState } from './state';
import type {
	AttachmentFetcher,
	BrowserSyncEnginePhase,
	BrowserSyncRemote,
	BrowserSyncStorage,
	FileChange,
	FileChangeCodec,
	OpenedFileChange,
	ReconcileResult,
	ResolveConflictResult,
	SyncConflict,
	SyncConflictReason,
	SyncConflictResolution,
	SyncState,
	SyncStateMigration,
	SyncStateStore,
} from './types';

/** Maximum number of operations accepted by one push request. */
export const MAX_PUSH_BATCH = 100;

/** Options accepted by {@link BrowserSyncEngine}. */
export interface BrowserSyncEngineOptions {
	/** Local replica boundary. */
	storage: BrowserSyncStorage;
	/** Encrypted sync service boundary. */
	remote: BrowserSyncRemote;
	/** Encrypted operation boundary. */
	codec: FileChangeCodec;
	/** Durable state boundary. */
	state: SyncStateStore;
	/**
	 * Downloads and decrypts version-3 attachments. Without it, a version-3
	 * change cannot be applied and reconcile fails with
	 * {@link BrowserSyncEngineErrorCode.AttachmentUnavailable} rather than
	 * writing an empty file.
	 */
	attachmentFetcher?: AttachmentFetcher;
	/** Clock used to timestamp conflicts. Defaults to `Date.now`. */
	now?: () => number;
	/** Called once on a revoked transition so the host can clear key material. */
	onRevoked?: (error: unknown) => void | Promise<void>;
	/** Called when an older durable state is upgraded or legacy conflicts are dropped. */
	onStateMigration?: (migration: SyncStateMigration) => void;
}

function requireOperationId(operation: EncryptedOperation): string {
	const operationId = (operation as { operationId?: unknown }).operationId;
	if (typeof operationId !== 'string' || operationId.length === 0) {
		throw new BrowserSyncEngineError(
			BrowserSyncEngineErrorCode.InvalidOperation,
			'An operation was missing a valid operation id',
		);
	}
	return operationId;
}

function isAttachmentBlob(value: unknown): boolean {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	const blob = value as Record<string, unknown>;
	return (
		typeof blob.id === 'string' &&
		typeof blob.revision === 'string' &&
		typeof blob.size === 'number' &&
		Number.isSafeInteger(blob.size) &&
		blob.size > 0 &&
		typeof blob.plaintextSize === 'number' &&
		Number.isSafeInteger(blob.plaintextSize) &&
		blob.plaintextSize >= 0 &&
		blob.plaintextSize < blob.size
	);
}

function validateOpenedFileChange(change: OpenedFileChange): void {
	const invalid =
		typeof change !== 'object' ||
		change === null ||
		typeof change.workspaceId !== 'string' ||
		typeof change.objectId !== 'string' ||
		!Number.isSafeInteger(change.epoch) ||
		change.epoch < 1 ||
		typeof change.path !== 'string' ||
		change.path.length === 0 ||
		(change.previousPath !== null && typeof change.previousPath !== 'string') ||
		(change.baseRevision !== null && typeof change.baseRevision !== 'string') ||
		(change.content !== null && !(change.content instanceof Uint8Array)) ||
		(change.blob !== undefined && !isAttachmentBlob(change.blob));
	if (invalid) {
		throw new BrowserSyncEngineError(
			BrowserSyncEngineErrorCode.InvalidOperation,
			'An opened file change had an unexpected shape',
		);
	}
}

/**
 * Reconciles one local browser replica against the encrypted sync service.
 *
 * The engine is not safe to share across concurrent `reconcile` calls; the host
 * should serialize calls per replica (for example behind the same Web Lock used
 * by `BrowserWorkspaceStorage`).
 */
export class BrowserSyncEngine {
	readonly #storage: BrowserSyncStorage;
	readonly #remote: BrowserSyncRemote;
	readonly #codec: FileChangeCodec;
	readonly #state: SyncStateStore;
	readonly #attachmentFetcher: AttachmentFetcher | undefined;
	readonly #now: () => number;
	readonly #onRevoked: ((error: unknown) => void | Promise<void>) | undefined;
	readonly #onStateMigration:
		((migration: SyncStateMigration) => void) | undefined;
	#phase: BrowserSyncEnginePhase = 'active';

	constructor(options: BrowserSyncEngineOptions) {
		this.#storage = options.storage;
		this.#remote = options.remote;
		this.#codec = options.codec;
		this.#state = options.state;
		this.#attachmentFetcher = options.attachmentFetcher;
		this.#now = options.now ?? Date.now;
		this.#onRevoked = options.onRevoked;
		this.#onStateMigration = options.onStateMigration;
	}

	/** The current lifecycle phase. */
	get phase(): BrowserSyncEnginePhase {
		return this.#phase;
	}

	/** True once the engine has been locked by revocation. */
	get locked(): boolean {
		return this.#phase === 'locked';
	}

	/**
	 * Seal a file change into the durable outbox. Returns only after the outbox
	 * has been persisted; no network call is made here.
	 */
	async enqueueFileChange(change: FileChange): Promise<EncryptedOperation> {
		this.#assertActive('enqueue a file change');
		const operation = await this.#seal(change);
		const state = await this.#readState();
		state.outbox = [...state.outbox, operation];
		await this.#recordEnqueuedBaseline(state, change);
		await this.#writeState(state);
		return operation;
	}

	/**
	 * Flush the outbox, then pull and apply remote operations up to the current
	 * remote head.
	 *
	 * The outbox is flushed in order in batches of at most
	 * {@link MAX_PUSH_BATCH}; operations are removed only after a successful
	 * push. On a push failure the outbox is left intact and a typed
	 * {@link BrowserSyncEngineError} is thrown. The durable cursor advances only
	 * after every operation in a page has been applied or recorded as a
	 * conflict.
	 */
	async reconcile(): Promise<ReconcileResult> {
		this.#assertActive('reconcile');
		let state = await this.#readState();
		if (!isCursor(state.cursor)) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.InvalidCursor,
				'The stored sync cursor was not a canonical decimal string',
			);
		}

		const pushed = await this.#flushOutbox(state);
		const conflicts: SyncConflict[] = [];
		let applied = 0;
		let hasMore = true;

		while (hasMore) {
			const page = await this.#pull(state.cursor);
			this.#validatePage(page, state.cursor);

			for (const operation of page.operations) {
				const operationId = requireOperationId(operation);
				if (
					state.conflicts.some((entry) => entry.operationId === operationId)
				) {
					continue;
				}
				const change = await this.#open(operation);
				const conflict = await this.#apply(
					operation,
					operationId,
					change,
					state,
				);
				if (conflict === null) applied += 1;
				else {
					state.conflicts = [...state.conflicts, conflict];
					conflicts.push(conflict);
				}
				await this.#writeState(state);
			}

			state.cursor = page.cursor;
			await this.#writeState(state);
			hasMore = page.hasMore;
		}

		return {
			pushed,
			applied,
			conflicts,
			cursor: state.cursor,
			hasMore,
		};
	}

	/**
	 * Resolve one recorded conflict, making the user's choice durable.
	 *
	 * `remote` re-opens the stored encrypted operation through the codec and
	 * force-applies it to local storage, bypassing the original `baseRevision`
	 * guard because the user chose the remote bytes. `local` seals the current
	 * local bytes for the conflict path as a fresh operation and enqueues it in
	 * the outbox; when the path no longer exists locally, a deletion is
	 * enqueued. Either way the conflict is removed only after the resolution
	 * work completes, and the durable state is written once.
	 *
	 * A resolution that cannot be performed throws a typed
	 * {@link BrowserSyncEngineError} and leaves the conflict in place; success is
	 * never fabricated. An unknown `operationId` is rejected with
	 * {@link BrowserSyncEngineErrorCode.ConflictNotFound}.
	 */
	async resolveConflict(
		operationId: string,
		choice: SyncConflictResolution,
	): Promise<ResolveConflictResult> {
		this.#assertActive('resolve a conflict');
		const state = await this.#readState();
		const index = state.conflicts.findIndex(
			(entry) => entry.operationId === operationId,
		);
		if (index < 0) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.ConflictNotFound,
				'No recorded conflict matched the operation id',
			);
		}
		const resolved = state.conflicts[index]!;

		if (choice === 'remote') {
			await this.#applyRemoteConflict(resolved, state);
		} else if (choice === 'local') {
			await this.#applyLocalConflict(resolved, state);
		} else {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.ResolveFailed,
				'A conflict resolution choice was not recognized',
			);
		}

		state.conflicts = state.conflicts.filter(
			(entry) => entry.operationId !== operationId,
		);
		await this.#writeState(state);
		return { resolved, remaining: state.conflicts };
	}

	/**
	 * Diff the local replica against the synchronized baseline.
	 *
	 * Every local path whose revision differs from `pushedRevisions`, every local
	 * path absent from the baseline, and every `knownPaths` entry no longer on
	 * disk is returned as a {@link FileChange}. Unchanged files are omitted.
	 * Results are sorted by path. The caller seals and enqueues them.
	 */
	async snapshotLocalChanges(): Promise<FileChange[]> {
		const state = await this.#readState();
		const localPaths = await this.#storage.list();
		const local = new Set(localPaths);
		const changes: FileChange[] = [];

		for (const path of [...localPaths].sort()) {
			const current = await this.#storage.read(path);
			if (current === null) continue;
			const pushedRevision = state.pushedRevisions[path];
			if (pushedRevision === current.revision) continue;
			changes.push({
				path,
				previousPath: null,
				baseRevision: pushedRevision ?? null,
				content: current.bytes,
			});
		}

		for (const path of [...state.knownPaths].sort()) {
			if (local.has(path)) continue;
			changes.push({
				path,
				previousPath: null,
				baseRevision: state.pushedRevisions[path] ?? null,
				content: null,
			});
		}

		return changes.sort((left, right) => left.path.localeCompare(right.path));
	}

	/**
	 * Lock the engine. Calls `onRevoked` so the host can clear key material and
	 * refuses further reconciliation. Persisted remote replicas are left for the
	 * caller to purge.
	 */
	async lock(reason?: unknown): Promise<void> {
		await this.#enterLocked(
			reason ??
				new BrowserSyncEngineError(
					BrowserSyncEngineErrorCode.Revoked,
					'The browser sync engine was locked',
				),
		);
	}

	async #seal(change: FileChange): Promise<EncryptedOperation> {
		try {
			return await this.#codec.sealFileChange(change);
		} catch (error) {
			if (isRevokedError(error)) throw await this.#toRevoked(error);
			throw error;
		}
	}

	async #open(operation: EncryptedOperation): Promise<OpenedFileChange> {
		let change: OpenedFileChange;
		try {
			change = await this.#codec.openFileChange(operation);
		} catch (error) {
			if (isRevokedError(error)) throw await this.#toRevoked(error);
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.InvalidOperation,
				'An operation could not be opened',
				{ cause: error },
			);
		}
		validateOpenedFileChange(change);
		return change;
	}

	async #pull(cursor: string): Promise<SyncPage> {
		try {
			return await this.#remote.pull(cursor);
		} catch (error) {
			if (isRevokedError(error)) throw await this.#toRevoked(error);
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.PullFailed,
				'Pulling operations failed; the cursor was not advanced',
				{ cause: error },
			);
		}
	}

	async #flushOutbox(state: SyncState): Promise<number> {
		let pushed = 0;
		while (state.outbox.length > 0) {
			const batch = state.outbox.slice(0, MAX_PUSH_BATCH);
			let sequences: string[];
			try {
				const response = await this.#remote.push(batch);
				sequences = response?.sequences;
				if (!Array.isArray(sequences) || sequences.length !== batch.length) {
					throw new BrowserSyncEngineError(
						BrowserSyncEngineErrorCode.InvalidResponse,
						'A push response did not match the pushed batch',
					);
				}
			} catch (error) {
				if (isRevokedError(error)) throw await this.#toRevoked(error);
				if (error instanceof BrowserSyncEngineError) throw error;
				throw new BrowserSyncEngineError(
					BrowserSyncEngineErrorCode.PushFailed,
					'Pushing the outbox failed; the outbox was left intact',
					{ cause: error },
				);
			}
			state.outbox = state.outbox.slice(batch.length);
			pushed += batch.length;
			await this.#writeState(state);
		}
		return pushed;
	}

	#validatePage(page: SyncPage, cursor: string): void {
		if (
			!page ||
			typeof page !== 'object' ||
			!Array.isArray(page.operations) ||
			typeof page.hasMore !== 'boolean' ||
			!isCursor(page.cursor)
		) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.InvalidResponse,
				'A pull response had an unexpected shape',
			);
		}
		if (BigInt(page.cursor) < BigInt(cursor)) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.InvalidResponse,
				'A pull response moved the cursor backwards',
			);
		}
		if (page.hasMore && page.cursor === cursor) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.InvalidResponse,
				'A pull response reported more pages without advancing the cursor',
			);
		}
	}

	async #apply(
		operation: EncryptedOperation,
		operationId: string,
		change: OpenedFileChange,
		state: SyncState,
	): Promise<SyncConflict | null> {
		const conflict = (
			reason: SyncConflictReason,
			currentRevision: string | null,
		): SyncConflict => ({
			operationId,
			objectId: change.objectId,
			path: change.path,
			reason,
			expectedRevision: change.baseRevision,
			currentRevision,
			previousPath: change.previousPath,
			detectedAt: this.#now(),
			operation,
		});

		const content = await this.#resolveContent(change);

		if (change.previousPath !== null) {
			if (content === null) return conflict('invalid_move', null);
			const source = await this.#storage.read(change.previousPath);
			const destination = await this.#storage.read(change.path);

			if (source === null) {
				if (destination !== null && bytesEqual(destination.bytes, content)) {
					this.#recordPresent(state, change.path, destination.revision);
					this.#recordAbsent(state, change.previousPath);
					return null;
				}
				return conflict(
					'missing_move_source',
					destination === null ? null : destination.revision,
				);
			}

			if (change.baseRevision === null) {
				if (destination !== null) {
					return conflict('occupied_destination', destination.revision);
				}
				try {
					await this.#storage.move({
						from: change.previousPath,
						to: change.path,
						expectedRevision: source.revision,
					});
				} catch {
					return conflict('storage_rejected', source.revision);
				}
				const written = await this.#storage.read(change.path);
				this.#recordAbsent(state, change.previousPath);
				this.#recordPresent(
					state,
					change.path,
					written === null ? source.revision : written.revision,
				);
				return null;
			}

			if (destination === null) {
				return conflict('missing_expected_file', null);
			}
			if (destination.revision !== change.baseRevision) {
				if (bytesEqual(destination.bytes, content)) {
					this.#recordAbsent(state, change.previousPath);
					this.#recordPresent(state, change.path, destination.revision);
					return null;
				}
				return conflict('revision_mismatch', destination.revision);
			}
			try {
				const written = await this.#storage.write({
					path: change.path,
					bytes: content,
					expectedRevision: change.baseRevision,
				});
				await this.#storage.delete({
					path: change.previousPath,
					expectedRevision: source.revision,
				});
				this.#recordAbsent(state, change.previousPath);
				this.#recordPresent(state, change.path, written.revision);
			} catch {
				return conflict('storage_rejected', destination.revision);
			}
			return null;
		}

		if (content !== null) {
			const current = await this.#storage.read(change.path);
			if (change.baseRevision === null) {
				if (current !== null) {
					if (bytesEqual(current.bytes, content)) {
						this.#recordPresent(state, change.path, current.revision);
						return null;
					}
					return conflict('unexpected_file', current.revision);
				}
				try {
					const written = await this.#storage.write({
						path: change.path,
						bytes: content,
						expectedRevision: null,
					});
					this.#recordPresent(state, change.path, written.revision);
				} catch {
					return conflict('storage_rejected', null);
				}
				return null;
			}

			if (current === null) {
				return conflict('missing_expected_file', null);
			}
			if (current.revision !== change.baseRevision) {
				if (bytesEqual(current.bytes, content)) {
					this.#recordPresent(state, change.path, current.revision);
					return null;
				}
				return conflict('revision_mismatch', current.revision);
			}
			try {
				const written = await this.#storage.write({
					path: change.path,
					bytes: content,
					expectedRevision: change.baseRevision,
				});
				this.#recordPresent(state, change.path, written.revision);
			} catch {
				return conflict('storage_rejected', current.revision);
			}
			return null;
		}

		const current = await this.#storage.read(change.path);
		if (change.baseRevision === null) {
			if (current === null) {
				this.#recordAbsent(state, change.path);
				return null;
			}
			return conflict('unexpected_file', current.revision);
		}
		if (current === null) {
			this.#recordAbsent(state, change.path);
			return null;
		}
		if (current.revision !== change.baseRevision) {
			return conflict('revision_mismatch', current.revision);
		}
		try {
			await this.#storage.delete({
				path: change.path,
				expectedRevision: change.baseRevision,
			});
			this.#recordAbsent(state, change.path);
		} catch {
			return conflict('storage_rejected', current.revision);
		}
		return null;
	}

	/**
	 * Resolve the plaintext the change should write.
	 *
	 * A version-1/2 change carries its bytes inline. A version-3 change carries
	 * only a signed blob descriptor, so the injected {@link AttachmentFetcher}
	 * supplies the decrypted bytes; a missing fetcher, an unavailable blob, or a
	 * plaintext whose length disagrees with the descriptor is a typed error, so
	 * an absent attachment is never applied as an empty file.
	 */
	async #resolveContent(change: OpenedFileChange): Promise<Uint8Array | null> {
		if (change.content !== null || change.blob === undefined) {
			return change.content;
		}
		const fetcher = this.#attachmentFetcher;
		if (fetcher === undefined) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.AttachmentUnavailable,
				'No attachment fetcher was configured for a version-3 change',
			);
		}
		let content: Uint8Array;
		try {
			content = await fetcher.fetch(change);
		} catch (error) {
			if (isRevokedError(error)) throw await this.#toRevoked(error);
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.AttachmentUnavailable,
				'Fetching the attachment for a version-3 change failed',
				{ cause: error },
			);
		}
		if (
			!(content instanceof Uint8Array) ||
			content.length !== change.blob.plaintextSize
		) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.AttachmentUnavailable,
				'The attachment plaintext did not match its signed size',
			);
		}
		return content;
	}

	/**
	 * Force-apply the stored remote operation for a conflict. The original
	 * `baseRevision` guard is intentionally dropped: the user chose the remote
	 * bytes, so a present local file is overwritten and a moved source is
	 * removed regardless of its revision. State is mutated in memory only; the
	 * caller persists it after removing the conflict.
	 */
	async #applyRemoteConflict(
		conflict: SyncConflict,
		state: SyncState,
	): Promise<void> {
		const change = await this.#open(conflict.operation);
		if (change.path !== conflict.path) {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.ResolveFailed,
				'The stored operation did not match the conflict path',
			);
		}
		try {
			const content = await this.#resolveContent(change);
			if (content === null) {
				if (change.previousPath !== null) {
					await this.#forceDelete(change.previousPath);
					this.#recordAbsent(state, change.previousPath);
				}
				await this.#forceDelete(change.path);
				this.#recordAbsent(state, change.path);
			} else {
				const written = await this.#storage.write({
					path: change.path,
					bytes: content,
				});
				this.#recordPresent(state, change.path, written.revision);
				if (
					change.previousPath !== null &&
					change.previousPath !== change.path
				) {
					await this.#forceDelete(change.previousPath);
					this.#recordAbsent(state, change.previousPath);
				}
			}
		} catch (error) {
			if (isRevokedError(error)) throw await this.#toRevoked(error);
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.ResolveFailed,
				'Applying the remote operation failed; the conflict was kept',
				{ cause: error },
			);
		}
	}

	/** Delete a path if it is present, without an expected-revision guard. */
	async #forceDelete(path: string): Promise<void> {
		const current = await this.#storage.read(path);
		if (current !== null) {
			await this.#storage.delete({ path });
		}
	}

	/**
	 * Make the local bytes win by sealing them as a fresh operation and
	 * enqueueing it. The change is based on the conflicting operation's
	 * expected revision, so it is a sibling write that supersedes the remote
	 * operation. A path that no longer exists locally becomes a deletion.
	 */
	async #applyLocalConflict(
		conflict: SyncConflict,
		state: SyncState,
	): Promise<void> {
		const current = await this.#storage.read(conflict.path);
		const change: FileChange = {
			path: conflict.path,
			previousPath: null,
			baseRevision: conflict.expectedRevision,
			content: current === null ? null : current.bytes,
		};
		const operation = await this.#seal(change);
		state.outbox = [...state.outbox, operation];
		await this.#recordEnqueuedBaseline(state, change);
	}

	async #recordEnqueuedBaseline(
		state: SyncState,
		change: FileChange,
	): Promise<void> {
		if (change.previousPath !== null) {
			this.#recordAbsent(state, change.previousPath);
		}
		if (change.content === null) {
			this.#recordAbsent(state, change.path);
			return;
		}
		const current = await this.#storage.read(change.path);
		if (current !== null) {
			this.#recordPresent(state, change.path, current.revision);
		}
	}

	#recordPresent(state: SyncState, path: string, revision: string): void {
		state.pushedRevisions[path] = revision;
		if (!state.knownPaths.includes(path)) {
			state.knownPaths = [...state.knownPaths, path].sort();
		}
	}

	#recordAbsent(state: SyncState, path: string): void {
		delete state.pushedRevisions[path];
		state.knownPaths = state.knownPaths.filter((entry) => entry !== path);
	}

	async #readState(): Promise<SyncState> {
		const onMigration = this.#onStateMigration;
		return cloneSyncState(
			validateSyncState(
				await this.#state.read(),
				onMigration ? { onMigration } : {},
			),
		);
	}

	async #writeState(state: SyncState): Promise<void> {
		await this.#state.write(cloneSyncState(state));
	}

	async #toRevoked(error: unknown): Promise<BrowserSyncEngineError> {
		await this.#enterLocked(error);
		return new BrowserSyncEngineError(
			BrowserSyncEngineErrorCode.Revoked,
			'The device or workspace access was revoked',
			{ cause: error },
		);
	}

	async #enterLocked(error: unknown): Promise<void> {
		if (this.#phase === 'locked') return;
		this.#phase = 'locked';
		try {
			await this.#onRevoked?.(error);
		} catch {
			// Clearing key material must not mask the revoked transition.
		}
	}

	#assertActive(action: string): void {
		if (this.#phase === 'locked') {
			throw new BrowserSyncEngineError(
				BrowserSyncEngineErrorCode.Locked,
				`Cannot ${action} while the engine is locked`,
			);
		}
	}
}
