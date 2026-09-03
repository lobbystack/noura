import type { AiProviderStreamFrame } from '@noura/ai';
import type {
	AiStreamFrame,
	AiToolDefinition,
	AiTransportMessage,
	ChatMessage,
} from '@noura/workspace';

type PiMessage = { role?: unknown; content?: unknown };
type PiContentPart = {
	type?: unknown;
	text?: unknown;
	id?: unknown;
	callId?: unknown;
	toolCallId?: unknown;
	name?: unknown;
	arguments?: unknown;
	content?: unknown;
};

function nativeContent(content: unknown): AiTransportMessage['content'] {
	if (typeof content === 'string')
		return content ? [{ type: 'text', text: content }] : [];
	if (!Array.isArray(content)) return [];
	return content.flatMap((value): AiTransportMessage['content'] => {
		if (!value || typeof value !== 'object') return [];
		const part = value as PiContentPart;
		if (part.type === 'text' && typeof part.text === 'string' && part.text)
			return [{ type: 'text', text: part.text }];
		const callId =
			typeof part.callId === 'string'
				? part.callId
				: typeof part.id === 'string'
					? part.id
					: typeof part.toolCallId === 'string'
						? part.toolCallId
						: null;
		if (part.type === 'toolCall' && callId && typeof part.name === 'string')
			return [
				{
					type: 'toolCall',
					callId,
					name: part.name,
					arguments: part.arguments ?? {},
				},
			];
		if (part.type === 'toolResult' && callId) {
			const result =
				typeof part.content === 'string'
					? part.content
					: typeof part.text === 'string'
						? part.text
						: '';
			return result
				? [
						{
							type: 'toolResult',
							callId,
							...(typeof part.name === 'string' ? { name: part.name } : {}),
							content: result,
						},
					]
				: [];
		}
		return [];
	});
}

function jsonValue(content: string): unknown {
	try {
		return JSON.parse(content);
	} catch {
		return {};
	}
}

/** Converts Pi's transient message shape into the narrow native stream contract. */
export function transportMessages(messages: unknown[]): AiTransportMessage[] {
	return messages.flatMap((value) => {
		if (!value || typeof value !== 'object') return [];
		const { role, content } = value as PiMessage;
		if (
			role !== 'system' &&
			role !== 'user' &&
			role !== 'assistant' &&
			role !== 'tool'
		)
			return [];
		const parts = nativeContent(content);
		const compatibleParts =
			role === 'tool'
				? parts.filter((part) => part.type === 'toolResult')
				: parts;
		return compatibleParts.length ? [{ role, content: compatibleParts }] : [];
	});
}

/** Builds the ordered native request from Pi's composed system prompt and history. */
export function nativeTransportMessages(
	systemPrompt: string,
	rehydration: Parameters<typeof rehydratedTransportMessages>[0],
	messages: unknown[],
): AiTransportMessage[] {
	return [
		...(systemPrompt.trim()
			? [
					{
						role: 'system' as const,
						content: [{ type: 'text' as const, text: systemPrompt }],
					},
				]
			: []),
		...rehydratedTransportMessages(rehydration),
		...transportMessages(messages),
	];
}

/** Rebuilds provider history from canonical chat messages, never UI state. */
export function rehydratedTransportMessages(
	messages: ReadonlyArray<{
		kind: ChatMessage['kind'];
		status: ChatMessage['status'];
		content: string;
		toolCallId?: string | null;
		toolName?: string | null;
	}>,
): AiTransportMessage[] {
	return messages.flatMap<AiTransportMessage>((message) => {
		if (!message.content || message.status !== 'completed') return [];
		if (message.kind === 'user')
			return [
				{ role: 'user', content: [{ type: 'text', text: message.content }] },
			];
		if (message.kind === 'assistant')
			return [
				{
					role: 'assistant',
					content: [{ type: 'text', text: message.content }],
				},
			];
		if (message.kind === 'context-summary')
			return [
				{ role: 'system', content: [{ type: 'text', text: message.content }] },
			];
		if (message.kind === 'tool-call' && message.toolCallId && message.toolName)
			return [
				{
					role: 'assistant',
					content: [
						{
							type: 'toolCall',
							callId: message.toolCallId,
							name: message.toolName,
							arguments: jsonValue(message.content),
						},
					],
				},
			];
		if (message.kind === 'tool-result' && message.toolCallId)
			return [
				{
					role: 'tool',
					content: [
						{
							type: 'toolResult',
							callId: message.toolCallId,
							...(message.toolName ? { name: message.toolName } : {}),
							content: message.content,
						},
					],
				},
			];
		return [];
	});
}

/** Removes executable plugin code before definitions cross the native boundary. */
export function nativeToolDefinitions(
	tools: ReadonlyArray<{
		definition: {
			name: string;
			description: string;
			inputSchema: unknown;
		};
	}>,
): AiToolDefinition[] {
	return tools.map(({ definition }) => ({
		name: definition.name,
		description: definition.description,
		inputSchema: definition.inputSchema,
	}));
}

/** Projects native stream events into the controller's provider stream contract. */
export function providerStreamFrame(
	frame: Pick<AiStreamFrame, 'operationId' | 'event'>,
	sequence: number,
): AiProviderStreamFrame | null {
	if (frame.event.type === 'started' || frame.event.type === 'error')
		return null;
	if (frame.event.type === 'textDelta')
		return {
			operationId: frame.operationId,
			sequence,
			kind: 'delta',
			text: frame.event.text,
		};
	if (frame.event.type === 'toolCall')
		return {
			operationId: frame.operationId,
			sequence,
			kind: 'tool-call',
			toolCallId: frame.event.call.callId,
			toolName: frame.event.call.name,
			input: frame.event.call.arguments as Record<string, unknown>,
		};
	return {
		operationId: frame.operationId,
		sequence,
		kind: frame.event.type === 'cancelled' ? 'aborted' : 'done',
	};
}

export function messageLabel(message: ChatMessage): string {
	if (message.kind === 'tool-call')
		return `Tool call: ${message.toolName ?? 'unknown'}`;
	if (message.kind === 'tool-result')
		return `Tool result: ${message.toolName ?? 'unknown'}`;
	if (message.kind === 'context-summary') return 'Context summary';
	return message.kind === 'assistant' ? 'Noura' : 'You';
}

export function chatTitle(message: string): string {
	const firstLine = message.trim().split('\n')[0] ?? '';
	return Array.from(firstLine).slice(0, 64).join('') || 'New chat';
}
