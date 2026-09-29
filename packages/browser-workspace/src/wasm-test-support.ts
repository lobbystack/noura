import {
	BrowserStorageError,
	BrowserWorkspaceStorage,
	type BrowserStorageFileSystem,
	type BrowserStorageLock,
} from '@noura/browser-storage';
import {
	loadWorkspaceFormat,
	type WorkspaceFormat,
} from '@noura/workspace-format-wasm';
import type { CoreEvent } from '@noura/shared';
import {
	BrowserWorkspaceServer,
	type BrowserWorkspaceRegistry,
	type WorkspaceChangeChannel,
	type WorkspaceChangeMessage,
} from './worker';

const wasmDirectory = new URL(
	'../../workspace-format-wasm/wasm/',
	import.meta.url,
);

/** The built canonical format, or null when `bun run wasm:build` has not run. */
export async function loadBuiltFormat(): Promise<WorkspaceFormat | null> {
	const binary = Bun.file(
		new URL('workspace_format_wasm_bg.wasm', wasmDirectory),
	);
	if (!(await binary.exists())) return null;
	return loadWorkspaceFormat(
		new URL('workspace_format_wasm.js', wasmDirectory).href,
		await binary.arrayBuffer(),
	);
}

/** An in-memory stand-in for one OPFS directory tree. */
export class MemoryFileSystem implements BrowserStorageFileSystem {
	readonly files = new Map<string, Uint8Array>();
	readonly directories = new Set<string>();

	async read(path: string, maxBytes?: number) {
		const value = this.files.get(path);
		if (value && maxBytes !== undefined && value.byteLength > maxBytes)
			throw new BrowserStorageError('snapshot_too_large', 'too large');
		return value?.slice() ?? null;
	}
	async write(path: string, bytes: Uint8Array) {
		this.files.set(path, bytes.slice());
	}
	async remove(path: string) {
		return this.files.delete(path);
	}
	async makeDirectory(path: string) {
		this.directories.add(path);
	}
	async removeDirectory(path: string, options: { recursive?: boolean } = {}) {
		const prefix = `${path}/`;
		const nested = [...this.files.keys(), ...this.directories].some((key) =>
			key.startsWith(prefix),
		);
		if (nested && !options.recursive) throw new Error('directory not empty');
		for (const key of [...this.files.keys()])
			if (key.startsWith(prefix)) this.files.delete(key);
		for (const key of [...this.directories])
			if (key.startsWith(prefix)) this.directories.delete(key);
		return this.directories.delete(path);
	}
	async list(path: string) {
		const prefix = path.length === 0 ? '' : `${path}/`;
		const entries = new Map<string, 'file' | 'directory'>();
		for (const key of this.directories)
			if (key.startsWith(prefix))
				entries.set(key.slice(prefix.length).split('/')[0]!, 'directory');
		for (const key of this.files.keys()) {
			if (!key.startsWith(prefix)) continue;
			const rest = key.slice(prefix.length);
			const slash = rest.indexOf('/');
			entries.set(
				slash === -1 ? rest : rest.slice(0, slash),
				slash === -1 ? 'file' : 'directory',
			);
		}
		return [...entries].map(([name, kind]) => ({ name, kind }));
	}
}

const lock: BrowserStorageLock = { run: (operation) => operation() };

/** One workspace directory per ID, shared by every server built on it. */
export function memoryRegistry(format: WorkspaceFormat) {
	const directories = new Map<string, MemoryFileSystem>();
	const storageFor = (id: string) =>
		new BrowserWorkspaceStorage({
			fileSystem: directories.get(id)!,
			format,
			lock,
		});
	const registry: BrowserWorkspaceRegistry = {
		async createFresh(id) {
			if (directories.has(id))
				throw new BrowserStorageError('workspace_not_empty', 'exists');
			directories.set(id, new MemoryFileSystem());
			return storageFor(id);
		},
		async open(id) {
			return directories.has(id) ? storageFor(id) : null;
		},
		async list() {
			return [...directories.keys()].map((id) => ({
				id,
				storage: storageFor(id),
			}));
		},
	};
	return { registry, directories };
}

/** Channels that deliver to every other channel of the same workspace. */
export function memoryChannels() {
	const open = new Map<string, Set<MemoryChannel>>();
	class MemoryChannel implements WorkspaceChangeChannel {
		readonly #listeners = new Set<
			(event: MessageEvent<WorkspaceChangeMessage>) => void
		>();
		constructor(readonly name: string) {
			const peers = open.get(name) ?? new Set();
			peers.add(this);
			open.set(name, peers);
		}
		postMessage(message: WorkspaceChangeMessage) {
			for (const peer of open.get(this.name) ?? [])
				if (peer !== this)
					for (const listener of peer.#listeners)
						listener({ data: structuredClone(message) } as MessageEvent);
		}
		addEventListener(
			_type: 'message',
			listener: (event: MessageEvent<WorkspaceChangeMessage>) => void,
		) {
			this.#listeners.add(listener);
		}
		close() {
			open.get(this.name)?.delete(this);
		}
	}
	return (id: string) => new MemoryChannel(id);
}

export function createServer(
	format: WorkspaceFormat,
	registry: BrowserWorkspaceRegistry,
	options: {
		now?: () => string;
		openChannel?: (id: string) => WorkspaceChangeChannel | null;
	} = {},
) {
	const events: CoreEvent[] = [];
	const server = new BrowserWorkspaceServer({
		format,
		registry,
		...(options.now ? { now: options.now } : {}),
		openChannel: options.openChannel ?? (() => null),
		onEvent: (event) => events.push(event),
	});
	return { server, events };
}
