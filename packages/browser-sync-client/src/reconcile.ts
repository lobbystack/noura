/**
 * One reconcile pass over the injected engine boundaries.
 *
 * File changes are routed to the object that owns their path and sealed with
 * that object's key through `@noura/browser-sync`'s file-change codec; the
 * engine from `@noura/browser-sync-engine` handles the outbox, push, pull, and
 * conflicts.
 */

import {
	attachmentObjectIdFromPath,
	BrowserSyncError,
	BrowserSyncErrorCode,
	createFileChangeCodec,
	encodeBase64,
} from '@noura/browser-sync';
import {
	BrowserSyncEngine,
	type BrowserSyncEngineOptions,
	type FileChange,
	type FileChangeCodec as EngineFileChangeCodec,
	type ReconcileResult,
	type ResolveConflictResult,
	type SyncConflictResolution,
} from '@noura/browser-sync-engine';
import { createBrowserAttachmentFetcher } from './attachment-fetcher';
import type {
	BrowserSyncObjectBinding,
	BrowserSyncReconcileInput,
	BrowserSyncWorkspaceBinding,
} from './types';

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
export function resolveObjectOwner(
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

export function createReconcileEngine(input: BrowserSyncReconcileInput) {
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
