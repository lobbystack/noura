/**
 * Adapters between a browser workspace replica and the engine's storage
 * boundary.
 *
 * {@link createWorkerWorkspaceStorage} adapts the workspace worker's raw file
 * operations and hides reserved roots. {@link createWorkspaceSyncStorage} maps
 * that replica onto {@link BrowserSyncStorage} while keeping the engine's
 * unguarded conflict writes and deletes working.
 */

import { BrowserSyncError, BrowserSyncErrorCode } from '@noura/browser-sync';
import type { WorkspaceStorageLike } from './storage';
import type { BrowserSyncStorage } from './types';

/**
 * Raw canonical file operations of a browser workspace replica. This matches
 * the file operations of `BrowserWorkspaceFiles` in `@noura/browser-workspace`
 * without importing it.
 */
export interface WorkspaceFileOperations {
	/** Every canonical path in the workspace, sorted. */
	list(): Promise<string[]>;
	/** Read one canonical file, or `null` when it does not exist. */
	read(path: string): Promise<{ bytes: Uint8Array; revision: string } | null>;
	/** Write a canonical file, requiring the given revision or its absence. */
	write(input: {
		path: string;
		bytes: Uint8Array;
		expectedRevision: string | null;
	}): Promise<{ revision: string }>;
	/** Move a canonical file, requiring the source revision and a free destination. */
	move(input: {
		from: string;
		to: string;
		expectedRevision: string;
		expectedDestinationRevision: string | null;
	}): Promise<unknown>;
	/** Delete a canonical file, requiring its current revision. */
	delete(input: { path: string; expectedRevision: string }): Promise<void>;
}

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
 * Adapt raw worker file operations to the engine's `WorkspaceStorageLike`.
 *
 * Only syncable paths are enumerated: the canonical sync schema rejects the
 * reserved `.noura`, `.git`, `node_modules`, and `target` roots, so the engine
 * must never see them in `list()`.
 */
export function createWorkerWorkspaceStorage(
	files: WorkspaceFileOperations,
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
