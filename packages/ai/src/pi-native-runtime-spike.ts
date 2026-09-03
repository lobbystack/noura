import { Agent, type StreamFn } from '@earendil-works/pi-agent-core';
import {
	createAssistantMessageEventStream,
	type AssistantMessage,
	type Model,
} from '@earendil-works/pi-ai';

const EMPTY_USAGE = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

const nativeModel: Model<'noura-native'> = {
	id: 'runtime-spike',
	name: 'Noura runtime spike',
	api: 'noura-native',
	provider: 'noura-native',
	baseUrl: '',
	reasoning: false,
	input: ['text'],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 8_192,
	maxTokens: 1_024,
};

export type PiRuntimeSpikeFrame = {
	operationId: string;
	sequence: number;
	kind: 'delta' | 'done' | 'aborted';
	text?: string;
};

export interface PiRuntimeSpikeTransport {
	stream(
		operationId: string,
		onFrame: (frame: PiRuntimeSpikeFrame) => void,
	): Promise<void>;
	cancel(operationId: string): Promise<boolean>;
}

function assistantMessage(
	content: AssistantMessage['content'],
	stopReason: AssistantMessage['stopReason'],
): AssistantMessage {
	return {
		role: 'assistant',
		content,
		api: nativeModel.api,
		provider: nativeModel.provider,
		model: nativeModel.id,
		usage: EMPTY_USAGE,
		stopReason,
		timestamp: Date.now(),
	};
}

function errorMessage(message: string, stopReason: 'aborted' | 'error') {
	return {
		...assistantMessage([], stopReason),
		errorMessage: message,
	};
}

function operationId() {
	const bytes = crypto.getRandomValues(new Uint8Array(16));
	bytes[6] = (bytes[6]! & 0x0f) | 0x40;
	bytes[8] = (bytes[8]! & 0x3f) | 0x80;
	const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0'));
	return `${hex.slice(0, 4).join('')}-${hex.slice(4, 6).join('')}-${hex.slice(6, 8).join('')}-${hex.slice(8, 10).join('')}-${hex.slice(10).join('')}`;
}

/**
 * Creates the Phase 0 Pi adapter. The injected transport is the only boundary
 * that can reach native code; the WebView neither handles provider credentials
 * nor initiates provider HTTP requests.
 */
export function createNativePiRuntimeSpike(transport: PiRuntimeSpikeTransport) {
	const streamFn: StreamFn = (_model, _context, options) => {
		const stream = createAssistantMessageEventStream();
		const id = operationId();
		let expectedSequence = 1;
		let text = '';
		let terminal = false;

		const abort = () => {
			void transport.cancel(id);
		};
		options?.signal?.addEventListener('abort', abort, { once: true });

		void transport
			.stream(id, (frame) => {
				if (terminal) return;
				if (frame.operationId !== id || frame.sequence !== expectedSequence) {
					terminal = true;
					stream.push({
						type: 'error',
						reason: 'error',
						error: errorMessage(
							'The native AI stream delivered an invalid frame sequence.',
							'error',
						),
					});
					return;
				}
				expectedSequence += 1;
				if (frame.kind === 'delta') {
					text += frame.text ?? '';
					const partial = assistantMessage([{ type: 'text', text }], 'pending');
					if (expectedSequence === 2) stream.push({ type: 'start', partial });
					stream.push({
						type: 'text_delta',
						contentIndex: 0,
						delta: frame.text ?? '',
						partial,
					});
					return;
				}
				terminal = true;
				if (frame.kind === 'aborted') {
					stream.push({
						type: 'error',
						reason: 'aborted',
						error: errorMessage(
							'The native AI stream was cancelled.',
							'aborted',
						),
					});
					return;
				}
				const completed = assistantMessage([{ type: 'text', text }], 'stop');
				stream.push({
					type: 'text_end',
					contentIndex: 0,
					content: text,
					partial: completed,
				});
				stream.push({ type: 'done', reason: 'stop', message: completed });
			})
			.catch(() => {
				if (terminal) return;
				terminal = true;
				stream.push({
					type: 'error',
					reason: 'error',
					error: errorMessage('The native AI stream could not start.', 'error'),
				});
			})
			.finally(() => {
				options?.signal?.removeEventListener('abort', abort);
				if (terminal) return;
				terminal = true;
				stream.push({
					type: 'error',
					reason: 'error',
					error: errorMessage(
						'The native AI stream ended unexpectedly.',
						'error',
					),
				});
			});

		return stream;
	};

	return new Agent({
		initialState: {
			model: nativeModel,
			systemPrompt: 'This is a native Tauri Channel compatibility probe.',
		},
		streamFn,
	});
}
