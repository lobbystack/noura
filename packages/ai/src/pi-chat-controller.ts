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
import { Type, type TSchema } from 'typebox';
import type {
	AiContextProvider,
	AiInstructionProvider,
	AiRegistry,
	AiRegistryEntry,
	AiSystemContextInput,
	AiToolDefinition,
} from './index';

const EMPTY_USAGE = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};
const MAX_SAFE_PARTIAL_TEXT = 16_384;

export type AiProviderStreamFrame =
	| {
			operationId: string;
			sequence: number;
			kind: 'delta';
			text?: string;
	  }
	| { operationId: string; sequence: number; kind: 'done' | 'aborted' }
	| {
			operationId: string;
			sequence: number;
			kind: 'tool-call';
			toolCallId: string;
			toolName: string;
			input: Record<string, unknown>;
	  };
export interface AiProviderStreamRequest {
	chatId: string;
	runId: string;
	operationId: string;
	systemPrompt: string;
	rehydration: AiChatRehydration;
	/** Provider transports receive Pi's current conversation without a workspace dependency. */
	messages: Array<unknown>;
}
export interface AiProviderStreamTransport {
	stream(
		input: AiProviderStreamRequest,
		onFrame: (frame: AiProviderStreamFrame) => void,
	): Promise<void>;
	cancel(operationId: string): Promise<boolean>;
}

export type AiChatMessageKind =
	'user' | 'assistant' | 'tool-call' | 'tool-result' | 'context-summary';
export type AiChatMessageStatus =
	'in-progress' | 'completed' | 'cancelled' | 'failed' | 'interrupted';
export interface AiChatMessageRef {
	id: string;
	revision: string;
}
export interface AiChatPersistenceWrite {
	chatRevision: string;
	message?: AiChatMessageRef;
}
export interface AiChatContextMessage {
	id: string;
	kind: AiChatMessageKind;
	status: AiChatMessageStatus;
	content: string;
}
export interface AiChatContextSummary {
	summarizesThroughMessageId: string;
	content: string;
}
export interface AiChatRehydration {
	chatRevision: string;
	messages: ReadonlyArray<AiChatContextMessage>;
	summaries: ReadonlyArray<AiChatContextSummary>;
}
export interface AiChatCompactionInput {
	chatId: string;
	runId: string;
	rehydration: AiChatRehydration;
}
export interface AiChatCompaction {
	/** Compact only when the original, immutable message history exceeds this count. */
	threshold: number;
	summarize(input: AiChatCompactionInput): Promise<AiChatContextSummary>;
}
export interface AiRevisionConflict {
	code: 'revision-conflict';
	message: string;
	expectedRevision: string;
	actualRevision?: string;
}
export function isAiRevisionConflict(
	error: unknown,
): error is AiRevisionConflict {
	return (
		typeof error === 'object' &&
		error !== null &&
		(error as { code?: unknown }).code === 'revision-conflict' &&
		typeof (error as { expectedRevision?: unknown }).expectedRevision ===
			'string'
	);
}

/**
 * Every method is a durable barrier: it must resolve only after its canonical
 * mutation commits. The controller never retries a rejected revision.
 */
export interface AiChatPersistence {
	rehydrate(input: { chatId: string }): Promise<AiChatRehydration>;
	appendUser(input: {
		chatId: string;
		runId: string;
		content: string;
		expectedChatRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	beginAssistant(input: {
		chatId: string;
		runId: string;
		providerId: string;
		modelId: string;
		expectedChatRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	beginToolCall(input: {
		chatId: string;
		runId: string;
		toolCallId: string;
		toolName: string;
		content: string;
		expectedChatRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	appendToolResult(input: {
		chatId: string;
		runId: string;
		toolCallId: string;
		toolName: string;
		content: string;
		expectedChatRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	finishToolCall(input: {
		chatId: string;
		messageId: string;
		content: string;
		status: Extract<
			AiChatMessageStatus,
			'completed' | 'failed' | 'interrupted'
		>;
		errorCode?: string;
		expectedChatRevision: string;
		expectedMessageRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	finishAssistant(input: {
		chatId: string;
		messageId: string;
		content: string;
		status: Extract<
			AiChatMessageStatus,
			'completed' | 'cancelled' | 'failed' | 'interrupted'
		>;
		errorCode?: string;
		expectedChatRevision: string;
		expectedMessageRevision: string;
	}): Promise<AiChatPersistenceWrite>;
	appendContextSummary(input: {
		chatId: string;
		runId: string;
		summarizesThroughMessageId: string;
		content: string;
		expectedChatRevision: string;
	}): Promise<AiChatPersistenceWrite>;
}

export interface AiToolConsentRequest {
	chatId: string;
	runId: string;
	toolCallId: string;
	tool: Pick<
		AiRegistryEntry<AiToolDefinition>,
		'owner' | 'revision' | 'risk'
	> & {
		name: string;
	};
	input: unknown;
}
export type AiToolConsentPolicy = 'automatic' | 'prompt' | 'deny';
export interface AiToolConsent {
	/** `automatic` authorizes a tool without calling `requestToolUse`. */
	policy?(input: AiToolConsentRequest): Promise<AiToolConsentPolicy>;
	requestToolUse(input: AiToolConsentRequest): Promise<boolean>;
}
export interface PiChatControllerOptions {
	model: Model<any>;
	providerTransport: AiProviderStreamTransport;
	persistence: AiChatPersistence;
	registry: AiRegistry;
	consent: AiToolConsent;
	compaction?: AiChatCompaction;
	systemPrompt?: string;
	operationId?: () => string;
	runId?: () => string;
}
export interface PiChatStartInput extends AiSystemContextInput {
	chatId: string;
	message: string;
	/** The configured provider identity, distinct from Pi's provider transport kind. */
	providerId?: string;
	/** Reuse this canonical user message instead of appending a second one. */
	resumeUserMessageId?: string;
}
export interface AiChatRunResult {
	chatId: string;
	runId: string;
	status: 'completed' | 'cancelled' | 'failed' | 'interrupted';
	text: string;
	error?: string;
	conflict?: AiRevisionConflict;
}
export interface AiChatRun {
	readonly chatId: string;
	readonly runId: string;
	readonly finished: Promise<AiChatRunResult>;
	cancel(): void;
}

interface ActiveRun {
	runId: string;
	contributions: Array<AiRegistryEntry<unknown>>;
	toolEntries: Array<AiRegistryEntry<AiToolDefinition>>;
	/** Provider call IDs are durable correlation keys and may occur once per run. */
	toolCallIds: Set<string>;
	operations: Set<string>;
	cancelled: boolean;
	failure?: string;
	interrupted?: { owner: string; errorCode: string };
	partialText: string;
	chatRevision?: string;
	assistant?: AiChatMessageRef;
	assistantFinishAttempted: boolean;
	agent?: Agent;
	unsubscribeRegistry?: () => void;
}

function assistantMessage(
	model: Model<any>,
	content: AssistantMessage['content'],
	stopReason: AssistantMessage['stopReason'],
): AssistantMessage {
	return {
		role: 'assistant',
		content,
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: EMPTY_USAGE,
		stopReason,
		timestamp: Date.now(),
	};
}

function errorMessage(
	model: Model<any>,
	message: string,
	stopReason: 'aborted' | 'error',
) {
	return {
		...assistantMessage(model, [], stopReason),
		errorMessage: message,
	};
}

/** Converts a plugin's JSON Schema at the Pi boundary, never in plugin-sdk. */
function toTypeBoxSchema(definition: AiToolDefinition): TSchema {
	if (definition.inputSchema === true) return Type.Any();
	if (definition.inputSchema === false) return Type.Never();
	return Type.Unsafe(definition.inputSchema);
}

function normalize(
	value: unknown,
): string | number | boolean | null | Array<unknown> | Record<string, unknown> {
	if (value === null || typeof value === 'string' || typeof value === 'boolean')
		return value;
	if (typeof value === 'number')
		return Number.isFinite(value) ? value : String(value);
	if (Array.isArray(value)) return value.map(normalize);
	if (typeof value === 'object') {
		const result: Record<string, unknown> = {};
		for (const key of Object.keys(value).sort()) {
			result[key] = normalize((value as Record<string, unknown>)[key]);
		}
		return result;
	}
	return String(value);
}

function normalizedContent(value: unknown): string {
	return JSON.stringify(normalize(value));
}

function safeText(text: string): string {
	return text.replaceAll('\0', '').slice(0, MAX_SAFE_PARTIAL_TEXT);
}

function errorText(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

function isNonEmptySingleLineString(value: unknown): value is string {
	return (
		typeof value === 'string' && value.length > 0 && !/[\p{Cc}]/u.test(value)
	);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function assistantText(agent: Agent): string {
	const message = [...agent.state.messages]
		.reverse()
		.find((value) => value.role === 'assistant');
	if (!message || !('content' in message)) return '';
	return message.content
		.filter((part) => part.type === 'text')
		.map((part) => part.text)
		.join('');
}

function operationId() {
	const bytes = crypto.getRandomValues(new Uint8Array(16));
	bytes[6] = (bytes[6]! & 0x0f) | 0x40;
	bytes[8] = (bytes[8]! & 0x3f) | 0x80;
	const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0'));
	return `${hex.slice(0, 4).join('')}-${hex.slice(4, 6).join('')}-${hex.slice(6, 8).join('')}-${hex.slice(8, 10).join('')}-${hex.slice(10).join('')}`;
}

function entryOrder<T>(left: AiRegistryEntry<T>, right: AiRegistryEntry<T>) {
	const leftId = (left.definition as { id?: string }).id ?? '';
	const rightId = (right.definition as { id?: string }).id ?? '';
	return left.owner.localeCompare(right.owner) || leftId.localeCompare(rightId);
}

export async function buildSystemPrompt(
	basePrompt: string,
	input: AiSystemContextInput,
	registry: AiRegistry,
): Promise<string> {
	const instructions = registry.instructionEntries().sort(entryOrder) as Array<
		AiRegistryEntry<AiInstructionProvider>
	>;
	const contexts = registry.contextEntries().sort(entryOrder) as Array<
		AiRegistryEntry<AiContextProvider>
	>;
	const instructionText = await Promise.all(
		instructions.map(async (entry) => {
			const content = await entry.definition.provide(input);
			if (!registry.isCurrent(entry)) return '';
			const text = Array.isArray(content) ? content.join('\n') : content;
			return text
				? `## Instructions: ${entry.owner}/${entry.definition.id}\n${text}`
				: '';
		}),
	);
	const contextText = await Promise.all(
		contexts.map(async (entry) => {
			const values = await entry.definition.provide(input);
			if (!registry.isCurrent(entry)) return '';
			const body = values
				.map((value) => `### ${value.title}\n${value.content}`)
				.join('\n\n');
			return body
				? `## Context: ${entry.owner}/${entry.definition.id}\n${body}`
				: '';
		}),
	);
	return [basePrompt, ...instructionText, ...contextText]
		.filter(Boolean)
		.join('\n\n');
}

/**
 * Transport-neutral Pi coordinator. It creates a fresh Agent for every active
 * chat run; all provider, consent, registry, and durable storage work is
 * injected by the caller.
 */
export class PiChatController {
	#options: PiChatControllerOptions;
	#runs = new Map<string, ActiveRun>();

	constructor(options: PiChatControllerOptions) {
		this.#options = options;
	}
	start(input: PiChatStartInput): AiChatRun {
		if (this.#runs.has(input.chatId))
			throw new Error(`AI chat already has an active run: ${input.chatId}`);
		const runId = (this.#options.runId ?? operationId)();
		const toolEntries = this.#options.registry.toolEntries();
		const run: ActiveRun = {
			runId,
			toolEntries,
			contributions: [
				...toolEntries,
				...this.#options.registry.contextEntries(),
				...this.#options.registry.instructionEntries(),
			],
			toolCallIds: new Set(),
			operations: new Set(),
			cancelled: false,
			partialText: '',
			assistantFinishAttempted: false,
		};
		run.unsubscribeRegistry = this.#options.registry.subscribe(() => {
			const disabled = run.contributions.find(
				(entry) => !this.#options.registry.isCurrent(entry),
			);
			if (disabled) this.#interruptForDisabledPlugin(run, disabled.owner);
		});
		this.#runs.set(input.chatId, run);

		const finished = this.#run(input, run);
		return {
			chatId: input.chatId,
			runId,
			finished,
			cancel: () => this.#cancelRun(run),
		};
	}
	active(chatId: string): boolean {
		return this.#runs.has(chatId);
	}
	#cancelRun(run: ActiveRun) {
		run.cancelled = true;
		if (run.agent) run.agent.abort();
		else {
			for (const id of run.operations)
				void this.#options.providerTransport.cancel(id).catch(() => {});
		}
	}
	#interruptForDisabledPlugin(run: ActiveRun, owner: string) {
		if (run.interrupted) return;
		run.interrupted = { owner, errorCode: 'plugin-disabled' };
		if (run.agent) run.agent.abort();
		else {
			for (const id of run.operations)
				void this.#options.providerTransport.cancel(id).catch(() => {});
		}
	}
	#toolsFor(chatId: string, run: ActiveRun): Array<AgentTool> {
		return run.toolEntries.map((entry) => ({
			name: entry.definition.name,
			label: entry.definition.name,
			description: entry.definition.description,
			parameters: toTypeBoxSchema(entry.definition),
			execute: (toolCallId, params) =>
				this.#executeTool(chatId, run, entry, toolCallId, params),
		}));
	}
	async #executeTool(
		chatId: string,
		run: ActiveRun,
		entry: AiRegistryEntry<AiToolDefinition>,
		toolCallId: string,
		input: unknown,
	) {
		if (this.#isStopped(run)) throw new Error('AI chat run was cancelled');
		if (!this.#options.registry.isCurrent(entry)) {
			this.#interruptForDisabledPlugin(run, entry.owner);
			throw new Error(`Plugin disabled: ${entry.owner}`);
		}
		const tool = {
			name: entry.definition.name,
			owner: entry.owner,
			revision: entry.revision,
			risk: entry.risk,
		};
		const intent = normalizedContent({ tool, input });
		const begun = await this.#options.persistence.beginToolCall({
			chatId,
			runId: run.runId,
			toolCallId,
			toolName: entry.definition.name,
			content: intent,
			expectedChatRevision: this.#chatRevision(run),
		});
		run.chatRevision = begun.chatRevision;
		const call = this.#messageRef(begun, 'tool call');
		const request: AiToolConsentRequest = {
			chatId,
			runId: run.runId,
			toolCallId,
			tool,
			input,
		};
		const finish = async (
			value: unknown,
			status: Extract<
				AiChatMessageStatus,
				'completed' | 'failed' | 'interrupted'
			>,
			errorCode?: string,
		) => {
			try {
				const content = normalizedContent(value);
				const result = await this.#options.persistence.appendToolResult({
					chatId,
					runId: run.runId,
					toolCallId,
					toolName: entry.definition.name,
					content,
					expectedChatRevision: this.#chatRevision(run),
				});
				run.chatRevision = result.chatRevision;
				const completed = await this.#options.persistence.finishToolCall({
					chatId,
					messageId: call.id,
					// The tool-call record is the durable execution intent. The separate
					// tool-result record stores the outcome.
					content: intent,
					status,
					...(status === 'failed'
						? { errorCode: 'tool-failed' }
						: errorCode
							? { errorCode }
							: {}),
					expectedChatRevision: this.#chatRevision(run),
					expectedMessageRevision: call.revision,
				});
				run.chatRevision = completed.chatRevision;
				return {
					content: [{ type: 'text' as const, text: content }],
					details: normalize(value),
				};
			} catch (error) {
				run.failure = errorText(error);
				run.agent?.abort();
				throw error;
			}
		};
		if (!this.#options.registry.isCurrent(entry)) {
			await finish(
				{ error: 'plugin-disabled' },
				'interrupted',
				'plugin-disabled',
			);
			this.#interruptForDisabledPlugin(run, entry.owner);
			throw new Error(`Plugin disabled: ${entry.owner}`);
		}
		if (this.#isStopped(run)) {
			await finish({ error: 'cancelled' }, 'interrupted');
			throw new Error('AI chat run was cancelled');
		}
		const policy = (await this.#options.consent.policy?.(request)) ?? 'prompt';
		if (policy === 'deny') return finish({ denied: true }, 'completed');
		if (
			policy === 'prompt' &&
			!(await this.#options.consent.requestToolUse(request))
		)
			return finish({ denied: true }, 'completed');
		if (!this.#options.registry.isCurrent(entry)) {
			await finish(
				{ error: 'plugin-disabled' },
				'interrupted',
				'plugin-disabled',
			);
			this.#interruptForDisabledPlugin(run, entry.owner);
			throw new Error(`Plugin disabled: ${entry.owner}`);
		}
		if (this.#isStopped(run)) {
			await finish({ error: 'cancelled' }, 'interrupted');
			throw new Error('AI chat run was cancelled');
		}
		let value: unknown;
		try {
			value = await entry.definition.execute(input);
		} catch (error) {
			const interrupted = run.interrupted;
			await finish(
				{ error: { code: 'tool-failed', message: errorText(error) } },
				interrupted || run.cancelled ? 'interrupted' : 'failed',
				interrupted?.errorCode,
			);
			throw error;
		}
		if (!this.#options.registry.isCurrent(entry)) {
			await finish(
				{ error: 'plugin-disabled' },
				'interrupted',
				'plugin-disabled',
			);
			this.#interruptForDisabledPlugin(run, entry.owner);
			throw new Error(`Plugin disabled: ${entry.owner}`);
		}
		if (this.#isStopped(run)) {
			await finish(
				{ error: run.interrupted?.errorCode ?? 'cancelled' },
				'interrupted',
				run.interrupted?.errorCode,
			);
			throw new Error('AI chat run was cancelled');
		}
		return finish(value, 'completed');
	}
	#streamFor(
		chatId: string,
		run: ActiveRun,
		systemPrompt: string,
		rehydration: AiChatRehydration,
	): StreamFn {
		return (_model, context, options) => {
			const stream = createAssistantMessageEventStream();
			if (this.#isStopped(run) || options?.signal?.aborted) {
				stream.push({
					type: 'error',
					reason: 'aborted',
					error: errorMessage(
						this.#options.model,
						'The provider AI stream was cancelled.',
						'aborted',
					),
				});
				return stream;
			}
			const id = (this.#options.operationId ?? operationId)();
			run.operations.add(id);
			let expectedSequence = 1;
			let text = '';
			const toolCalls: Array<{
				type: 'toolCall';
				id: string;
				name: string;
				arguments: Record<string, unknown>;
			}> = [];
			let terminal = false;
			let started = false;
			let cancelRequested = false;
			const cancel = () => {
				if (cancelRequested) return;
				cancelRequested = true;
				void this.#options.providerTransport.cancel(id).catch(() => {});
			};
			const abort = () => {
				if (terminal) return;
				terminal = true;
				run.operations.delete(id);
				cancel();
				stream.push({
					type: 'error',
					reason: 'aborted',
					error: errorMessage(
						this.#options.model,
						'The provider AI stream was cancelled.',
						'aborted',
					),
				});
			};
			const start = (partial: AssistantMessage) => {
				if (!started) {
					started = true;
					stream.push({ type: 'start', partial });
				}
			};
			const fail = (message: string) => {
				terminal = true;
				run.operations.delete(id);
				cancel();
				stream.push({
					type: 'error',
					reason: 'error',
					error: errorMessage(this.#options.model, message, 'error'),
				});
			};
			options?.signal?.addEventListener('abort', abort, { once: true });
			void this.#options.providerTransport
				.stream(
					{
						chatId,
						runId: run.runId,
						operationId: id,
						systemPrompt,
						rehydration,
						messages: [...context.messages],
					},
					(frame) => {
						if (terminal) return;
						if (this.#isStopped(run)) {
							abort();
							return;
						}
						if (
							frame.operationId !== id ||
							frame.sequence !== expectedSequence
						) {
							fail(
								'The provider AI stream delivered an invalid frame sequence.',
							);
							return;
						}
						expectedSequence += 1;
						if (frame.kind === 'delta') {
							const delta = frame.text ?? '';
							text += delta;
							run.partialText = safeText(text);
							const partial = assistantMessage(
								this.#options.model,
								[{ type: 'text', text }],
								'pending',
							);
							start(partial);
							stream.push({
								type: 'text_delta',
								contentIndex: 0,
								delta,
								partial,
							});
							return;
						}
						if (frame.kind === 'aborted') {
							terminal = true;
							run.operations.delete(id);
							stream.push({
								type: 'error',
								reason: 'aborted',
								error: errorMessage(
									this.#options.model,
									'The provider AI stream was cancelled.',
									'aborted',
								),
							});
							return;
						}
						if (frame.kind === 'tool-call') {
							if (
								!isNonEmptySingleLineString(frame.toolCallId) ||
								!isNonEmptySingleLineString(frame.toolName) ||
								!isRecord(frame.input) ||
								run.toolCallIds.has(frame.toolCallId) ||
								!run.toolEntries.some(
									(entry) => entry.definition.name === frame.toolName,
								)
							) {
								fail('The provider AI stream delivered an invalid tool call.');
								return;
							}
							run.toolCallIds.add(frame.toolCallId);
							const toolCall = {
								type: 'toolCall' as const,
								id: frame.toolCallId,
								name: frame.toolName,
								arguments: frame.input,
							};
							// A provider may emit explanatory text before requesting a tool.
							// Pi indexes the tool after that text content part.
							const contentIndex = (text ? 1 : 0) + toolCalls.length;
							toolCalls.push(toolCall);
							const partial = assistantMessage(
								this.#options.model,
								[
									...(text ? ([{ type: 'text' as const, text }] as const) : []),
									...toolCalls,
								],
								'toolUse',
							);
							start(partial);
							stream.push({ type: 'toolcall_start', contentIndex, partial });
							stream.push({
								type: 'toolcall_end',
								contentIndex,
								toolCall,
								partial,
							});
							return;
						}
						terminal = true;
						run.operations.delete(id);
						if (toolCalls.length > 0) {
							const completed = assistantMessage(
								this.#options.model,
								[
									...(text ? ([{ type: 'text' as const, text }] as const) : []),
									...toolCalls,
								],
								'toolUse',
							);
							stream.push({
								type: 'done',
								reason: 'toolUse',
								message: completed,
							});
							return;
						}
						const completed = assistantMessage(
							this.#options.model,
							[{ type: 'text', text }],
							'stop',
						);
						start(completed);
						stream.push({
							type: 'text_end',
							contentIndex: 0,
							content: text,
							partial: completed,
						});
						stream.push({ type: 'done', reason: 'stop', message: completed });
					},
				)
				.catch(() => {
					if (terminal) return;
					terminal = true;
					run.operations.delete(id);
					stream.push({
						type: 'error',
						reason: 'error',
						error: errorMessage(
							this.#options.model,
							'The provider AI stream could not start.',
							'error',
						),
					});
				})
				.finally(() => {
					run.operations.delete(id);
					options?.signal?.removeEventListener('abort', abort);
					if (terminal) return;
					terminal = true;
					stream.push({
						type: 'error',
						reason: 'error',
						error: errorMessage(
							this.#options.model,
							'The provider AI stream ended unexpectedly.',
							'error',
						),
					});
				});
			return stream;
		};
	}
	async #run(
		input: PiChatStartInput,
		run: ActiveRun,
	): Promise<AiChatRunResult> {
		let error: unknown;
		try {
			let rehydration = await this.#options.persistence.rehydrate({
				chatId: input.chatId,
			});
			run.chatRevision = rehydration.chatRevision;
			this.#validateResumedUser(input, rehydration);
			if (this.#isStopped(run)) return this.#stoppedResult(input.chatId, run);
			if (
				this.#options.compaction &&
				rehydration.messages.filter(
					(message) => message.kind !== 'context-summary',
				).length > this.#options.compaction.threshold
			) {
				const summary = await this.#options.compaction.summarize({
					chatId: input.chatId,
					runId: run.runId,
					rehydration: {
						...rehydration,
						messages: [...rehydration.messages],
						summaries: [...rehydration.summaries],
					},
				});
				if (this.#isStopped(run)) return this.#stoppedResult(input.chatId, run);
				const persisted = await this.#options.persistence.appendContextSummary({
					chatId: input.chatId,
					runId: run.runId,
					summarizesThroughMessageId: summary.summarizesThroughMessageId,
					content: summary.content,
					expectedChatRevision: this.#chatRevision(run),
				});
				run.chatRevision = persisted.chatRevision;
				rehydration = await this.#options.persistence.rehydrate({
					chatId: input.chatId,
				});
				run.chatRevision = rehydration.chatRevision;
				this.#validateResumedUser(input, rehydration);
				if (this.#isStopped(run)) return this.#stoppedResult(input.chatId, run);
			}
			if (!input.resumeUserMessageId) {
				const user = await this.#options.persistence.appendUser({
					chatId: input.chatId,
					runId: run.runId,
					content: input.message,
					expectedChatRevision: this.#chatRevision(run),
				});
				run.chatRevision = user.chatRevision;
			}
			if (this.#isStopped(run)) return this.#stoppedResult(input.chatId, run);
			const systemPrompt = await buildSystemPrompt(
				this.#options.systemPrompt ?? '',
				input,
				this.#options.registry,
			);
			if (this.#isStopped(run)) return this.#stoppedResult(input.chatId, run);
			const begun = await this.#options.persistence.beginAssistant({
				chatId: input.chatId,
				runId: run.runId,
				providerId: input.providerId ?? this.#options.model.provider,
				modelId: this.#options.model.id,
				expectedChatRevision: this.#chatRevision(run),
			});
			run.chatRevision = begun.chatRevision;
			run.assistant = this.#messageRef(begun, 'assistant');
			if (this.#isStopped(run)) {
				const status = this.#terminalStatus(run);
				await this.#finishAssistant(input.chatId, run, status, '');
				return this.#result(input.chatId, run, status, '');
			}
			const agent = new Agent({
				initialState: {
					model: this.#options.model,
					systemPrompt,
					tools: this.#toolsFor(input.chatId, run),
				},
				streamFn: this.#streamFor(
					input.chatId,
					run,
					systemPrompt,
					this.#providerRehydration(input, rehydration),
				),
				toolExecution: 'sequential',
			});
			run.agent = agent;
			if (run.cancelled || run.interrupted) agent.abort();
			await agent.prompt(input.message);
			const agentError = run.failure ?? agent.state.errorMessage;
			const status = this.#terminalStatus(run, agentError);
			const text = safeText(assistantText(agent) || run.partialText);
			await this.#finishAssistant(
				input.chatId,
				run,
				status,
				text,
				status === 'interrupted'
					? run.interrupted?.errorCode
					: status === 'failed'
						? 'failed'
						: undefined,
			);
			return this.#result(input.chatId, run, status, text, agentError);
		} catch (caught) {
			error = caught;
			const status = this.#terminalStatus(run, errorText(caught));
			const text = safeText(
				run.agent ? assistantText(run.agent) : run.partialText,
			);
			if (run.assistant && !run.assistantFinishAttempted) {
				try {
					await this.#finishAssistant(
						input.chatId,
						run,
						status,
						text,
						status === 'interrupted'
							? run.interrupted?.errorCode
							: status === 'failed'
								? 'failed'
								: undefined,
					);
				} catch (finishError) {
					error = finishError;
				}
			}
			return this.#result(
				input.chatId,
				run,
				status,
				text,
				errorText(error),
				error,
			);
		} finally {
			for (const id of run.operations)
				void this.#options.providerTransport.cancel(id).catch(() => {});
			run.unsubscribeRegistry?.();
			this.#runs.delete(input.chatId);
		}
	}
	#chatRevision(run: ActiveRun): string {
		if (!run.chatRevision) throw new Error('Chat revision is unavailable');
		return run.chatRevision;
	}
	#isStopped(run: ActiveRun): boolean {
		return run.cancelled || Boolean(run.interrupted);
	}
	#stoppedResult(chatId: string, run: ActiveRun): AiChatRunResult {
		return this.#result(chatId, run, this.#terminalStatus(run), '');
	}
	#validateResumedUser(
		input: PiChatStartInput,
		rehydration: AiChatRehydration,
	) {
		if (!input.resumeUserMessageId) return;
		const matches = rehydration.messages.filter(
			(message) => message.id === input.resumeUserMessageId,
		);
		if (
			matches.length !== 1 ||
			matches[0]?.kind !== 'user' ||
			matches[0]?.status !== 'completed' ||
			matches[0]?.content !== input.message
		) {
			throw new Error(
				'The resumed user message is not canonical completed content.',
			);
		}
	}
	#providerRehydration(
		input: PiChatStartInput,
		rehydration: AiChatRehydration,
	): AiChatRehydration {
		if (!input.resumeUserMessageId) return rehydration;
		return {
			...rehydration,
			messages: rehydration.messages.filter(
				(message) => message.id !== input.resumeUserMessageId,
			),
		};
	}
	#messageRef(write: AiChatPersistenceWrite, label: string): AiChatMessageRef {
		if (!write.message)
			throw new Error(`Persistence did not return a ${label} revision`);
		return write.message;
	}
	#terminalStatus(
		run: ActiveRun,
		error?: string,
	): Extract<
		AiChatMessageStatus,
		'completed' | 'cancelled' | 'failed' | 'interrupted'
	> {
		if (run.interrupted) return 'interrupted';
		if (run.failure !== undefined) return 'failed';
		if (run.cancelled || error?.toLowerCase().includes('cancel'))
			return 'cancelled';
		return error ? 'failed' : 'completed';
	}
	async #finishAssistant(
		chatId: string,
		run: ActiveRun,
		status: Extract<
			AiChatMessageStatus,
			'completed' | 'cancelled' | 'failed' | 'interrupted'
		>,
		content: string,
		errorCode?: string,
	) {
		if (!run.assistant || run.assistantFinishAttempted) return;
		run.assistantFinishAttempted = true;
		const write = await this.#options.persistence.finishAssistant({
			chatId,
			messageId: run.assistant.id,
			content,
			status,
			...(errorCode ? { errorCode } : {}),
			expectedChatRevision: this.#chatRevision(run),
			expectedMessageRevision: run.assistant.revision,
		});
		run.chatRevision = write.chatRevision;
	}
	#result(
		chatId: string,
		run: ActiveRun,
		status: AiChatRunResult['status'],
		text: string,
		error?: string,
		rawError?: unknown,
	): AiChatRunResult {
		const conflict = isAiRevisionConflict(rawError) ? rawError : undefined;
		return {
			chatId,
			runId: run.runId,
			status,
			text,
			...(error ? { error } : {}),
			...(conflict ? { conflict } : {}),
		};
	}
}
