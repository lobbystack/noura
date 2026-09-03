import { expect, test } from 'bun:test';
import {
	createNativePiRuntimeSpike,
	type PiRuntimeSpikeFrame,
	type PiRuntimeSpikeTransport,
} from './pi-native-runtime-spike';

test('Pi receives ordered native channel frames through an injected transport', async () => {
	const transport: PiRuntimeSpikeTransport = {
		async stream(operationId, onFrame) {
			const frames: PiRuntimeSpikeFrame[] = [
				{ operationId, sequence: 1, kind: 'delta', text: 'Native ' },
				{ operationId, sequence: 2, kind: 'delta', text: 'stream works.' },
				{ operationId, sequence: 3, kind: 'done' },
			];
			frames.forEach(onFrame);
		},
		async cancel() {
			return true;
		},
	};
	const agent = createNativePiRuntimeSpike(transport);

	await agent.prompt('Verify the native stream.');

	expect(agent.state.messages.at(-1)).toMatchObject({
		role: 'assistant',
		content: [{ type: 'text', text: 'Native stream works.' }],
	});
});

test('Pi rejects out-of-order native channel frames', async () => {
	const transport: PiRuntimeSpikeTransport = {
		async stream(operationId, onFrame) {
			onFrame({ operationId, sequence: 2, kind: 'delta', text: 'late' });
		},
		async cancel() {
			return true;
		},
	};
	const agent = createNativePiRuntimeSpike(transport);

	await agent.prompt('Verify frame ordering.');

	expect(agent.state.errorMessage).toBe(
		'The native AI stream delivered an invalid frame sequence.',
	);
});

test('Pi cancellation calls the injected native transport', async () => {
	let cancelCalls = 0;
	let frame: ((value: PiRuntimeSpikeFrame) => void) | undefined;
	let operationId = '';
	let markStarted!: () => void;
	const started = new Promise<void>((resolve) => {
		markStarted = resolve;
	});
	const transport: PiRuntimeSpikeTransport = {
		async stream(id, onFrame) {
			operationId = id;
			frame = onFrame;
			markStarted();
			await new Promise<void>(() => {});
		},
		async cancel() {
			cancelCalls += 1;
			frame?.({ operationId, sequence: 1, kind: 'aborted' });
			return true;
		},
	};
	const agent = createNativePiRuntimeSpike(transport);
	const run = agent.prompt('Cancel the native stream.');
	await started;
	agent.abort();
	await run;

	expect(cancelCalls).toBe(1);
});
