import { createBrowserWorkspaceClient } from '@noura/browser-workspace';

export function startBrowserWorkspace() {
	const worker = new Worker(
		new URL('./browser-workspace.worker.ts', import.meta.url),
		{ type: 'module' },
	);
	const {
		client,
		plugins,
		transport,
		files,
		exportWorkspace,
		importWorkspace,
	} = createBrowserWorkspaceClient(worker);
	let disposed = false;
	const stop = () => {
		if (disposed) return;
		disposed = true;
		transport.dispose();
		void plugins.deactivateAll().catch(() => {});
		worker.terminate();
		worker.removeEventListener('error', stop);
		worker.removeEventListener('messageerror', stop);
	};
	worker.addEventListener('error', stop);
	worker.addEventListener('messageerror', stop);
	let rejectReady: (error: Error) => void;
	let timer: ReturnType<typeof setTimeout>;
	const cleanup = () => {
		clearTimeout(timer);
		worker.removeEventListener('message', message);
		worker.removeEventListener('error', failed);
	};
	const failed = () => {
		cleanup();
		stop();
		rejectReady(
			new Error(
				'Browser storage could not start. Use a secure origin and a browser with OPFS and Web Locks support.',
			),
		);
	};
	let resolveReady: () => void;
	const message = (event: MessageEvent) => {
		if (event.data?.type === 'ready') {
			cleanup();
			resolveReady();
		}
		if (event.data?.type === 'startup-error') {
			cleanup();
			stop();
			rejectReady(
				new Error(
					`Browser storage initialization failed (${event.data.stage}). Reload to retry.`,
				),
			);
		}
	};
	const ready = new Promise<void>((resolve, reject) => {
		resolveReady = resolve;
		rejectReady = reject;
		worker.addEventListener('message', message);
		worker.addEventListener('error', failed);
		timer = setTimeout(failed, 30_000);
	});
	return {
		client,
		plugins,
		files,
		exportWorkspace,
		importWorkspace,
		ready,
		async dispose() {
			cleanup();
			rejectReady(new Error('Browser workspace closed'));
			try {
				await plugins.deactivateAll();
			} finally {
				stop();
			}
		},
	};
}
