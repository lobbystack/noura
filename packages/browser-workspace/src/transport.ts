import type { CoreEvent, WorkspaceState } from '@noura/shared';
import {
	browserPluginCapabilities,
	createNouraClient,
	PluginRuntime,
	type CoreTransport,
	type NouraClient,
} from '@noura/workspace/browser';
import type {
	BrowserWorkerEndpoint,
	BrowserWorkerRequest,
	BrowserWorkerResponse,
} from './protocol';
import type { BrowserWorkspaceSnapshot } from './worker';

type Pending = {
	resolve(value: unknown): void;
	reject(reason: unknown): void;
};

/** A browser-safe CoreTransport backed by one dedicated workspace worker. */
export function createBrowserWorkerTransport(
	worker: BrowserWorkerEndpoint,
): CoreTransport & { dispose(): void } {
	let nextId = 0;
	let disposed = false;
	const pending = new Map<string, Pending>();
	const subscribers = new Set<(event: CoreEvent) => void>();
	const failAll = (error: Error) => {
		for (const request of pending.values()) request.reject(error);
		pending.clear();
	};
	const receive = (event: MessageEvent<BrowserWorkerResponse> | ErrorEvent) => {
		if (!('data' in event)) {
			failAll(new Error('The browser workspace worker stopped unexpectedly'));
			return;
		}
		const message = event.data;
		if (message.type === 'event') {
			for (const subscriber of subscribers) subscriber(message.event);
			return;
		}
		const request = pending.get(message.id);
		if (!request) return;
		pending.delete(message.id);
		if (message.ok) request.resolve(message.value);
		else request.reject(message.error);
	};
	worker.addEventListener('message', receive);
	worker.addEventListener('error', receive);
	worker.addEventListener('messageerror', receive);

	return {
		request<T>(
			command: string,
			payload: Record<string, unknown> = {},
		): Promise<T> {
			if (disposed)
				return Promise.reject(
					new Error('The browser workspace transport is disposed'),
				);
			const id = `browser-request-${nextId++}`;
			const message: BrowserWorkerRequest = {
				type: 'request',
				id,
				command,
				payload,
			};
			return new Promise<T>((resolve, reject) => {
				pending.set(id, { resolve, reject });
				try {
					worker.postMessage(message);
				} catch (error) {
					pending.delete(id);
					reject(error);
				}
			});
		},
		async subscribe(handler: (event: CoreEvent) => void) {
			if (disposed)
				throw new Error('The browser workspace transport is disposed');
			subscribers.add(handler);
			return () => subscribers.delete(handler);
		},
		dispose() {
			if (disposed) return;
			disposed = true;
			worker.removeEventListener('message', receive);
			worker.removeEventListener('error', receive);
			worker.removeEventListener('messageerror', receive);
			failAll(new Error('The browser workspace transport is disposed'));
			subscribers.clear();
		},
	};
}

/** A stored canonical file and its opaque content revision. */
export interface BrowserWorkspaceFile {
	revision: string;
	bytes: Uint8Array;
}

/**
 * Raw canonical file operations on the active browser workspace.
 *
 * Paths are validated by `BrowserWorkspaceStorage` in the worker, which also
 * enforces the expected revision and journals each mutation. Bytes cross the
 * worker boundary as canonical base64 and are decoded here into `Uint8Array`.
 */
export interface BrowserWorkspaceFiles {
	/** Every canonical path in the active workspace, sorted. */
	list(): Promise<string[]>;
	/** Read one canonical file, or `null` when it does not exist. */
	read(path: string): Promise<BrowserWorkspaceFile | null>;
	/** Write a canonical file, requiring the given revision or its absence. */
	write(input: {
		path: string;
		bytes: Uint8Array;
		expectedRevision: string | null;
	}): Promise<{ path: string; revision: string }>;
	/** Move a canonical file, requiring the source revision and a free destination. */
	move(input: {
		from: string;
		to: string;
		expectedRevision: string;
		expectedDestinationRevision: string | null;
	}): Promise<{ path: string; revision: string }>;
	/** Delete a canonical file, requiring its current revision. */
	delete(input: { path: string; expectedRevision: string }): Promise<void>;
}

/** Build the raw canonical file operations over a worker transport. */
export function createBrowserWorkspaceFiles(
	transport: CoreTransport,
): BrowserWorkspaceFiles {
	return {
		list: () => transport.request<string[]>('files_list'),
		async read(path) {
			const value = await transport.request<{
				revision: string;
				bytes: string;
			} | null>('files_read', { path });
			return value === null
				? null
				: { revision: value.revision, bytes: decodeBase64(value.bytes) };
		},
		write: (input) =>
			transport.request<{ path: string; revision: string }>('files_write', {
				path: input.path,
				bytes: encodeBase64(input.bytes),
				expectedRevision: input.expectedRevision,
			}),
		move: (input) =>
			transport.request<{ path: string; revision: string }>('files_move', {
				from: input.from,
				to: input.to,
				expectedRevision: input.expectedRevision,
				expectedDestinationRevision: input.expectedDestinationRevision,
			}),
		async delete(input) {
			await transport.request('files_delete', {
				path: input.path,
				expectedRevision: input.expectedRevision,
			});
		},
	};
}

function encodeBase64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function decodeBase64(value: string): Uint8Array {
	const binary = atob(value);
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1)
		bytes[index] = binary.charCodeAt(index);
	return bytes;
}

/** Creates the normal typed Noura client over a browser worker transport. */
export function createBrowserWorkspaceClient(worker: BrowserWorkerEndpoint): {
	client: NouraClient;
	/**
	 * Capability-limited plugin runtime. It can activate notes, tasks, and
	 * projects only; it never grants browser plugins AI, file, search, or cache
	 * services.
	 */
	plugins: PluginRuntime;
	transport: CoreTransport & { dispose(): void };
	/** Raw canonical file operations on the active workspace. */
	files: BrowserWorkspaceFiles;
	/** Returns a lossless structured-clone snapshot; packaging it for download is UI work. */
	exportWorkspace(): Promise<BrowserWorkspaceSnapshot>;
	/** Imports into a new browser workspace with the snapshot's stable workspace ID. */
	importWorkspace(snapshot: BrowserWorkspaceSnapshot): Promise<WorkspaceState>;
} {
	const transport = createBrowserWorkerTransport(worker);
	const client = createNouraClient(transport);
	return {
		client,
		plugins: new PluginRuntime(client, {
			platform: 'web',
			supportedCapabilities: browserPluginCapabilities,
		}),
		transport,
		files: createBrowserWorkspaceFiles(transport),
		exportWorkspace: () =>
			transport.request<BrowserWorkspaceSnapshot>('workspace_export'),
		importWorkspace: (snapshot) =>
			transport.request<WorkspaceState>('workspace_import', { snapshot }),
	};
}
