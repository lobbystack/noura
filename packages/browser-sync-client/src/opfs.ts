/**
 * Origin-private (OPFS) stores for browser sync adapter state.
 *
 * Everything here lives under an adapter-owned directory outside canonical
 * workspace files, so it is never enumerated, exported, or synchronized. Only
 * wrapped key material is ever written.
 */

import { OpfsFileSystem } from '@noura/browser-storage';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	isIdentifier,
	parseBrowserSyncBindingRecord,
	type BrowserSyncBindingStore,
	type KeyStore,
	type WrappedKeyBundle,
} from '@noura/browser-sync';
import {
	createFileSystemSyncStateStore,
	type SyncStateStore,
} from '@noura/browser-sync-engine';
import type { BrowserSyncPluginMarker } from './plugin-migration';

/** Directory outside canonical workspace files that holds adapter-owned state. */
const ADAPTER_DIRECTORY = '.noura-adapter/browser-sync';

/**
 * The subset of `StorageManager` this module uses. `getDirectory` is optional
 * because it is not present in every browser or type library.
 */
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
 * Open the marker that records this browser's one-time sync plugin
 * migration for a local workspace. It lives in the adapter directory, so it
 * is never exported or synchronized. Returns `null` without OPFS.
 */
export async function openBrowserSyncPluginMarker(
	localWorkspaceId: string,
): Promise<BrowserSyncPluginMarker | null> {
	const directory = await openOpfsDirectory(`${ADAPTER_DIRECTORY}/plugin`);
	if (!directory) return null;
	const name = `${requireBundleId(localWorkspaceId)}.json`;
	return {
		async exists() {
			return (await readOpfsFile(directory, name)) !== null;
		},
		async write() {
			if (await readOpfsFile(directory, name)) return;
			const handle = await directory.getFileHandle(name, { create: true });
			const writable = await handle.createWritable();
			try {
				await writable.write(JSON.stringify({ version: 1 }));
				await writable.close();
			} catch (error) {
				await writable.abort().catch(() => {});
				throw error;
			}
		},
	};
}

/** Whether this browser holds a sync binding for a local workspace. */
export async function hasBrowserSyncBinding(
	localWorkspaceId: string,
): Promise<boolean> {
	const directory = await openOpfsDirectory(`${ADAPTER_DIRECTORY}/bindings`);
	if (!directory) return false;
	return (
		(await readOpfsFile(
			directory,
			`${requireBundleId(localWorkspaceId)}.json`,
		)) !== null
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
			return parseBrowserSyncBindingRecord(await file.text());
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
			// A workspace bound through the plugin has settled its plugin
			// state; the one-time migration must not revisit it.
			await (await openBrowserSyncPluginMarker(localWorkspaceId))?.write();
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
