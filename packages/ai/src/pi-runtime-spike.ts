import {
	Agent,
	type AgentTool,
	type StreamFn,
} from '@earendil-works/pi-agent-core';
import {
	createAssistantMessageEventStream,
	type AssistantMessage,
	type Model,
} from '@earendil-works/pi-ai';
import { Type } from 'typebox';

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

function emitText(text: string, signal: AbortSignal | undefined) {
	const stream = createAssistantMessageEventStream();
	const partial = assistantMessage([], 'pending');

	queueMicrotask(() => {
		if (signal?.aborted) {
			stream.push({
				type: 'error',
				reason: 'aborted',
				error: {
					...assistantMessage([], 'aborted'),
					errorMessage: 'The native stream was cancelled.',
				},
			});
			return;
		}
		stream.push({ type: 'start', partial });
		stream.push({ type: 'text_start', contentIndex: 0, partial });
		const completed = assistantMessage([{ type: 'text', text }], 'stop');
		stream.push({
			type: 'text_delta',
			contentIndex: 0,
			delta: text,
			partial: completed,
		});
		stream.push({
			type: 'text_end',
			contentIndex: 0,
			content: text,
			partial: completed,
		});
		stream.push({ type: 'done', reason: 'stop', message: completed });
	});

	return stream;
}

function emitToolCall(signal: AbortSignal | undefined) {
	const stream = createAssistantMessageEventStream();
	const partial = assistantMessage([], 'pending');
	const toolCall = {
		type: 'toolCall' as const,
		id: 'runtime-spike-call',
		name: 'workspace.echo',
		arguments: { message: 'native tool completed' },
	};

	queueMicrotask(() => {
		if (signal?.aborted) {
			stream.push({
				type: 'error',
				reason: 'aborted',
				error: {
					...assistantMessage([], 'aborted'),
					errorMessage: 'The native stream was cancelled.',
				},
			});
			return;
		}
		stream.push({ type: 'start', partial });
		stream.push({ type: 'toolcall_start', contentIndex: 0, partial });
		const completed = assistantMessage([toolCall], 'toolUse');
		stream.push({
			type: 'toolcall_end',
			contentIndex: 0,
			toolCall,
			partial: completed,
		});
		stream.push({ type: 'done', reason: 'toolUse', message: completed });
	});

	return stream;
}

/**
 * Browser-only Phase 0 probe for Pi Agent Core. Its stream function is deliberately
 * synthetic: the production implementation must replace it with the native Tauri
 * Channel adapter, where provider credentials remain outside the WebView.
 */
export function createPiRuntimeSpike() {
	const echoParameters = Type.Object({ message: Type.String() });
	const echoTool: AgentTool<typeof echoParameters> = {
		name: 'workspace.echo',
		label: 'Echo workspace result',
		description: 'Phase 0 in-memory tool used to verify Pi tool execution.',
		parameters: echoParameters,
		async execute(_toolCallId, params) {
			return {
				content: [{ type: 'text', text: params.message }],
				details: { echoed: params.message },
			};
		},
	};

	const streamFn: StreamFn = (_model, context, options) => {
		const hasToolResult = context.messages.some(
			(message) => message.role === 'toolResult',
		);
		if (hasToolResult)
			return emitText('The native tool result was received.', options?.signal);
		return emitToolCall(options?.signal);
	};

	return new Agent({
		initialState: {
			model: nativeModel,
			systemPrompt: 'This is a browser-only Pi runtime compatibility probe.',
			tools: [echoTool],
		},
		streamFn,
		toolExecution: 'sequential',
	});
}
