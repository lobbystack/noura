import { describe, expect, test } from 'bun:test';
import { createNouraClient } from '@noura/workspace';
import { createBrowserWorkspace } from './browser-workspace';

class TestWorker extends EventTarget {
	terminated = false;
	requests: Array<{ id: string; command: string }> = [];
	postMessage(message: { id: string; command: string }) {
		this.requests.push(message);
	}
	terminate() {
		this.terminated = true;
	}
	message(data: unknown) {
		this.dispatchEvent(new MessageEvent('message', { data }));
	}
}

function start() {
	const worker = new TestWorker();
	const workspace = createBrowserWorkspace(worker as unknown as Worker);
	return { worker, workspace, client: createNouraClient(workspace.transport) };
}

describe('browser workspace worker', () => {
	test('sends requests right away; the worker holds them until it starts', async () => {
		const { worker, workspace, client } = start();
		const request = client.workspaces.current();
		expect(worker.requests).toMatchObject([{ command: 'workspace_state' }]);
		worker.message({ type: 'ready' });
		await workspace.ready;
		worker.message({
			type: 'response',
			id: worker.requests[0]!.id,
			ok: true,
			value: { phase: 'idle', indexedFiles: 0, diagnostics: [] },
		});
		await expect(request).resolves.toMatchObject({ phase: 'idle' });
	});

	test('reports a browser that cannot store workspaces', async () => {
		const { worker, workspace } = start();
		worker.message({ type: 'startup-error', stage: 'storage' });
		await expect(workspace.ready).rejects.toThrow('can’t store workspaces');
	});

	test('a crashed worker rejects pending and later requests', async () => {
		const { worker, client } = start();
		const pending = client.notes.list();
		worker.dispatchEvent(new Event('error'));
		await expect(pending).rejects.toThrow();
		await expect(client.notes.list()).rejects.toThrow('disposed');
		expect(worker.terminated).toBe(true);
	});
});
