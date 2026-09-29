import {
	BROWSER_WORKSPACE_PATH_PREFIX,
	createBrowserWorkerTransport,
	createBrowserWorkspaceFiles,
	type BrowserWorkspaceFiles,
	type BrowserWorkspaceSnapshot,
} from '@noura/browser-workspace';
import type { CoreTransport, WorkspaceState } from '@noura/workspace';

/** The path a new browser workspace is created at. */
export const NEW_BROWSER_WORKSPACE_PATH = BROWSER_WORKSPACE_PATH_PREFIX;

export interface BrowserWorkspace {
	/** The transport the typed client runs on. */
	transport: CoreTransport;
	/** Raw canonical file operations, for browser sync. */
	files: BrowserWorkspaceFiles;
	/** Resolves once storage is ready; rejects when this browser can't store files. */
	ready: Promise<void>;
	exportWorkspace(): Promise<BrowserWorkspaceSnapshot>;
	importWorkspace(snapshot: BrowserWorkspaceSnapshot): Promise<WorkspaceState>;
}

let current: BrowserWorkspace | null = null;

/**
 * The one workspace worker for this tab. The typed client talks to it like it
 * talks to the desktop app; requests sent before it starts wait for it.
 */
export function getBrowserWorkspace(): BrowserWorkspace {
	current ??= createBrowserWorkspace(
		new Worker(new URL('./browser-workspace.worker.ts', import.meta.url), {
			type: 'module',
		}),
	);
	return current;
}

/** Wrap one started workspace worker. */
export function createBrowserWorkspace(
	worker: Worker,
): BrowserWorkspace & { dispose(): void } {
	const transport = createBrowserWorkerTransport(worker);
	// A crashed worker never answers again: fail fast instead of hanging.
	const crashed = () => {
		transport.dispose();
		worker.terminate();
	};
	worker.addEventListener('error', crashed);
	const ready = new Promise<void>((resolve, reject) => {
		const listen = (event: MessageEvent) => {
			if (event.data?.type === 'ready') resolve();
			else if (event.data?.type === 'startup-error')
				reject(new Error('This browser can’t store workspaces.'));
			else return;
			worker.removeEventListener('message', listen);
		};
		worker.addEventListener('message', listen);
	});
	// Callers that never await `ready` must not see an unhandled rejection.
	ready.catch(() => {});
	return {
		transport,
		files: createBrowserWorkspaceFiles(transport),
		ready,
		exportWorkspace: () =>
			transport.request<BrowserWorkspaceSnapshot>('workspace_export'),
		importWorkspace: (snapshot) =>
			transport.request<WorkspaceState>('workspace_import', { snapshot }),
		dispose() {
			worker.removeEventListener('error', crashed);
			crashed();
		},
	};
}

const LAST_WORKSPACE_KEY = 'noura.browser.last-workspace';

/** The browser workspace this tab opened last, to open it again on reload. */
export function lastBrowserWorkspace(): string | null {
	try {
		return localStorage.getItem(LAST_WORKSPACE_KEY);
	} catch {
		return null;
	}
}

export function rememberBrowserWorkspace(workspaceId: string) {
	try {
		localStorage.setItem(LAST_WORKSPACE_KEY, workspaceId);
	} catch {
		// Opening the workspace again by hand still works.
	}
}
