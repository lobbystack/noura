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
		exportWorkspace: () =>
			transport.request<BrowserWorkspaceSnapshot>('workspace_export'),
		importWorkspace: (snapshot) =>
			transport.request<WorkspaceState>('workspace_import', { snapshot }),
	};
}
