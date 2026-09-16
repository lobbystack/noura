import { afterEach, describe, expect, test } from 'bun:test';
import { startBrowserWorkspace } from './browser-workspace';

const OriginalWorker = globalThis.Worker;
class TestWorker extends EventTarget {
	static instance: TestWorker;
	terminated = false;
	requests: unknown[] = [];
	constructor() {
		super();
		TestWorker.instance = this;
	}
	postMessage(message: unknown) {
		this.requests.push(message);
	}
	terminate() {
		this.terminated = true;
	}
	message(data: unknown) {
		this.dispatchEvent(new MessageEvent('message', { data }));
	}
}
afterEach(() => {
	globalThis.Worker = OriginalWorker;
});
function start() {
	globalThis.Worker = TestWorker as unknown as typeof Worker;
	return startBrowserWorkspace();
}
describe('browser workspace worker lifecycle', () => {
	test('waits for readiness, uses the typed transport, and rejects pending calls on teardown', async () => {
		const runtime = start();
		const worker = TestWorker.instance;
		expect(worker.requests).toEqual([]);
		worker.message({ type: 'ready' });
		await runtime.ready;
		const request = runtime.client.workspaces.current();
		expect(worker.requests).toMatchObject([{ command: 'workspace_state' }]);
		runtime.dispose();
		await expect(request).rejects.toThrow('disposed');
		expect(worker.terminated).toBe(true);
		runtime.dispose();
	});
	test('startup failure terminates the worker instead of leaving a pending client', async () => {
		const runtime = start();
		TestWorker.instance.message({ type: 'startup-error', stage: 'WASM' });
		await expect(runtime.ready).rejects.toThrow('WASM');
		expect(TestWorker.instance.terminated).toBe(true);
		await expect(runtime.client.workspaces.current()).rejects.toThrow(
			'disposed',
		);
	});
	test('a crashed worker rejects subsequent requests too', async () => {
		const runtime = start();
		TestWorker.instance.message({ type: 'ready' });
		await runtime.ready;
		TestWorker.instance.dispatchEvent(new Event('error'));
		await expect(runtime.client.notes.list()).rejects.toThrow('disposed');
		runtime.dispose();
	});
});
