import { expect, test } from 'bun:test';
import type { Model } from '@earendil-works/pi-ai';
import {
	AiRegistry,
	buildSystemPrompt,
	PiChatController,
	type AiChatCompaction,
	type AiChatContextMessage,
	type AiChatContextSummary,
	type AiChatPersistence,
	type AiChatRehydration,
	type AiProviderStreamFrame,
	type AiProviderStreamTransport,
} from './index';

const model: Model<'noura-native'> = {
	id: 'test-model',
	name: 'Test model',
	api: 'noura-native',
	provider: 'noura-native',
	baseUrl: '',
	reasoning: false,
	input: ['text'],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 8_192,
	maxTokens: 1_024,
};

function durableHarness(
	input: {
		messages?: ReadonlyArray<AiChatContextMessage>;
		summaries?: ReadonlyArray<AiChatContextSummary>;
	} = {},
) {
	const events: Array<string> = [];
	const writes: Array<{ method: string; input: Record<string, unknown> }> = [];
	let revision = 0;
	let summaries = [...(input.summaries ?? [])];
	const next = (
		method: string,
		value: Record<string, unknown>,
		messageId?: string,
	) => {
		writes.push({ method, input: value });
		events.push(method);
		revision += 1;
		return {
			chatRevision: `chat-${revision}`,
			...(messageId
				? { message: { id: messageId, revision: `${messageId}-${revision}` } }
				: {}),
		};
	};
	const persistence: AiChatPersistence = {
		rehydrate: async () => {
			events.push('rehydrate');
			return {
				chatRevision: `chat-${revision}`,
				messages: [...(input.messages ?? [])],
				summaries: [...summaries],
			};
		},
		appendUser: async (value) => next('append-user', value),
		beginAssistant: async (value) =>
			next('begin-assistant', value, 'assistant-message'),
		beginToolCall: async (value) =>
			next('begin-tool-call', value, `tool-call-${value.toolCallId}`),
		appendToolResult: async (value) =>
			next('append-tool-result', value, `tool-result-${value.toolCallId}`),
		finishToolCall: async (value) => next('finish-tool-call', value),
		finishAssistant: async (value) => next('finish-assistant', value),
		appendContextSummary: async (value) => {
			summaries = [
				...summaries,
				{
					summarizesThroughMessageId: value.summarizesThroughMessageId,
					content: value.content,
				},
			];
			return next('append-context-summary', value, 'summary-message');
		},
	};
	return { events, persistence, writes };
}

function controller(
	persistence: AiChatPersistence,
	transport: AiProviderStreamTransport,
	registry = new AiRegistry(),
	options: Partial<{
		compaction: AiChatCompaction;
		consent: ConstructorParameters<typeof PiChatController>[0]['consent'];
	}> = {},
) {
	return new PiChatController({
		model,
		persistence,
		providerTransport: transport,
		registry,
		consent: options.consent ?? {
			policy: async () => 'automatic',
			requestToolUse: async () => true,
		},
		...(options.compaction ? { compaction: options.compaction } : {}),
		runId: () => 'run-1',
		operationId: (() => {
			let count = 0;
			return () => `operation-${++count}`;
		})(),
	});
}

function prompt(controller: PiChatController, chatId = 'chat-1') {
	return controller.start({
		chatId,
		workspaceId: 'workspace-1',
		message: 'Hello.',
	});
}

test('system context has deterministic instruction then context ordering', async () => {
	const registry = new AiRegistry();
	registry.registerContextProvider(
		{
			id: 'tasks.today',
			provide: async () => [{ title: 'Today', content: 'Ship tests.' }],
		},
		{ owner: 'tasks' },
	);
	registry.registerInstructionProvider(
		{ id: 'voice', provide: async () => 'Be concise.' },
		{ owner: 'notes' },
	);
	registry.registerInstructionProvider(
		{ id: 'safety', provide: async () => 'Ask before deleting.' },
		{ owner: 'calendar' },
	);

	expect(
		await buildSystemPrompt(
			'Base rules.',
			{ workspaceId: 'workspace-1' },
			registry,
		),
	).toBe(
		'Base rules.\n\n## Instructions: calendar/safety\nAsk before deleting.\n\n## Instructions: notes/voice\nBe concise.\n\n## Context: tasks/tasks.today\n### Today\nShip tests.',
	);
});

test('disabled plugin context is omitted before system context reaches the provider', async () => {
	const registry = new AiRegistry();
	let release!: () => void;
	const ready = new Promise<void>((resolve) => {
		release = resolve;
	});
	const dispose = registry.registerInstructionProvider(
		{
			id: 'transient',
			provide: async () => {
				await ready;
				return 'This must not reach the provider.';
			},
		},
		{ owner: 'disabled-plugin' },
	);
	const system = buildSystemPrompt(
		'Base.',
		{ workspaceId: 'workspace-1' },
		registry,
	);
	dispose();
	release();
	expect(await system).toBe('Base.');
});

test('durably persists user and in-progress assistant before provider streaming', async () => {
	const { events, persistence, writes } = durableHarness();
	const transport: AiProviderStreamTransport = {
		async stream(input, onFrame) {
			events.push(`stream:${input.runId}`);
			onFrame({
				operationId: input.operationId,
				sequence: 1,
				kind: 'delta',
				text: 'Saved.',
			});
			onFrame({ operationId: input.operationId, sequence: 2, kind: 'done' });
		},
		async cancel() {
			return true;
		},
	};
	const run = prompt(controller(persistence, transport));
	expect(await run.finished).toMatchObject({
		status: 'completed',
		text: 'Saved.',
		runId: 'run-1',
	});
	expect(events).toEqual([
		'rehydrate',
		'append-user',
		'begin-assistant',
		'stream:run-1',
		'finish-assistant',
	]);
	expect(writes[0]?.input).toMatchObject({
		runId: 'run-1',
		expectedChatRevision: 'chat-0',
	});
	expect(writes[1]?.input).toMatchObject({
		expectedChatRevision: 'chat-1',
		providerId: 'noura-native',
		modelId: 'test-model',
	});
	expect(writes.at(-1)?.input).toMatchObject({
		status: 'completed',
		content: 'Saved.',
		expectedChatRevision: 'chat-2',
	});
});

test('persists the configured provider identity rather than Pi provider kind', async () => {
	const { persistence, writes } = durableHarness();
	const result = await controller(persistence, {
		async stream(input, onFrame) {
			onFrame({ operationId: input.operationId, sequence: 1, kind: 'done' });
		},
		cancel: async () => true,
	}).start({
		chatId: 'chat-1',
		workspaceId: 'workspace-1',
		message: 'Hello.',
		providerId: 'configured-provider',
	}).finished;
	expect(result.status).toBe('completed');
	expect(
		writes.find((write) => write.method === 'begin-assistant')?.input,
	).toMatchObject({ providerId: 'configured-provider', modelId: 'test-model' });
});

test('tool calls persist intent, deterministic result, and completion before continuing', async () => {
	const { events, persistence, writes } = durableHarness();
	const registry = new AiRegistry();
	let executions = 0;
	registry.registerTool(
		{
			name: 'tasks.write',
			description: 'Write a task.',
			inputSchema: { type: 'object' },
			risk: 'high',
			execute: async () => {
				events.push('execute-tool');
				executions += 1;
				return { b: 2, a: 1 };
			},
		},
		{ owner: 'tasks' },
	);
	let streams = 0;
	const transport: AiProviderStreamTransport = {
		async stream(input, onFrame) {
			streams += 1;
			events.push(`stream-${streams}`);
			if (streams === 1) {
				onFrame({
					operationId: input.operationId,
					sequence: 1,
					kind: 'tool-call',
					toolCallId: 'call-1',
					toolName: 'tasks.write',
					input: { taskId: 'task-1' },
				});
				onFrame({ operationId: input.operationId, sequence: 2, kind: 'done' });
				return;
			}
			onFrame({
				operationId: input.operationId,
				sequence: 1,
				kind: 'delta',
				text: 'Done.',
			});
			onFrame({ operationId: input.operationId, sequence: 2, kind: 'done' });
		},
		async cancel() {
			return true;
		},
	};
	const run = prompt(
		controller(persistence, transport, registry, {
			consent: {
				policy: async () => {
					events.push('automatic-policy');
					return 'automatic';
				},
				requestToolUse: async () => {
					throw new Error('Automatic policy must not prompt');
				},
			},
		}),
	);
	expect(await run.finished).toMatchObject({
		status: 'completed',
		text: 'Done.',
	});
	expect(executions).toBe(1);
	expect(events).toContain('automatic-policy');
	expect(events.indexOf('begin-tool-call')).toBeLessThan(
		events.indexOf('execute-tool'),
	);
	expect(events.indexOf('append-tool-result')).toBeLessThan(
		events.indexOf('finish-tool-call'),
	);
	expect(
		writes.find((write) => write.method === 'begin-tool-call')?.input,
	).toMatchObject({
		toolName: 'tasks.write',
		content:
			'{"input":{"taskId":"task-1"},"tool":{"name":"tasks.write","owner":"tasks","revision":1,"risk":"high"}}',
	});
	expect(
		writes.find((write) => write.method === 'append-tool-result')?.input,
	).toMatchObject({ content: '{"a":1,"b":2}' });
	expect(
		writes.find((write) => write.method === 'finish-tool-call')?.input,
	).toMatchObject({ status: 'completed' });
});

test('provider text remains before tool calls in the next provider request', async () => {
	const { persistence } = durableHarness();
	const registry = new AiRegistry();
	registry.registerTool(
		{
			name: 'tasks.inspect',
			description: 'Inspect a task.',
			inputSchema: { type: 'object' },
			execute: async () => ({ title: 'Ship tests' }),
		},
		{ owner: 'tasks' },
	);
	let streams = 0;
	let followUpMessages: Array<{
		role?: string;
		content?: Array<{ type?: string; text?: string; name?: string }>;
	}> = [];
	const result = await prompt(
		controller(
			persistence,
			{
				async stream(input, onFrame) {
					streams += 1;
					if (streams === 1) {
						onFrame({
							operationId: input.operationId,
							sequence: 1,
							kind: 'delta',
							text: 'I will inspect it.',
						});
						onFrame({
							operationId: input.operationId,
							sequence: 2,
							kind: 'tool-call',
							toolCallId: 'call-1',
							toolName: 'tasks.inspect',
							input: {},
						});
						onFrame({
							operationId: input.operationId,
							sequence: 3,
							kind: 'done',
						});
						return;
					}
					followUpMessages = input.messages as typeof followUpMessages;
					onFrame({
						operationId: input.operationId,
						sequence: 1,
						kind: 'delta',
						text: 'Finished.',
					});
					onFrame({
						operationId: input.operationId,
						sequence: 2,
						kind: 'done',
					});
				},
				cancel: async () => true,
			},
			registry,
		),
	).finished;

	expect(result).toMatchObject({ status: 'completed', text: 'Finished.' });
	expect(
		followUpMessages.find((message) => message.role === 'assistant')?.content,
	).toEqual([
		{ type: 'text', text: 'I will inspect it.' },
		expect.objectContaining({ type: 'toolCall', name: 'tasks.inspect' }),
	]);
});

test('revision conflicts surface without retrying or starting a provider stream', async () => {
	const { events, persistence, writes } = durableHarness();
	let beginAttempts = 0;
	const conflicting: AiChatPersistence = {
		...persistence,
		beginAssistant: async () => {
			beginAttempts += 1;
			throw {
				code: 'revision-conflict',
				message: 'Chat changed externally.',
				expectedRevision: 'chat-1',
				actualRevision: 'chat-external',
			};
		},
	};
	let streams = 0;
	const result = await prompt(
		controller(conflicting, {
			stream: async () => {
				streams += 1;
			},
			cancel: async () => true,
		}),
	).finished;
	expect(result).toMatchObject({
		status: 'failed',
		conflict: { expectedRevision: 'chat-1', actualRevision: 'chat-external' },
	});
	expect(beginAttempts).toBe(1);
	expect(streams).toBe(0);
	expect(events).toEqual(['rehydrate', 'append-user']);
	expect(writes).toHaveLength(1);
});

test('compaction persists a summary and rehydrates it without changing original messages', async () => {
	const messages: ReadonlyArray<AiChatContextMessage> = Object.freeze([
		{ id: 'm1', kind: 'user', status: 'completed', content: 'One' },
		{ id: 'm2', kind: 'assistant', status: 'completed', content: 'Two' },
		{ id: 'm3', kind: 'user', status: 'completed', content: 'Three' },
	]);
	const original = structuredClone(messages);
	const { events, persistence } = durableHarness({ messages });
	let rehydration: AiChatRehydration | undefined;
	const transport: AiProviderStreamTransport = {
		async stream(input, onFrame) {
			rehydration = input.rehydration;
			onFrame({
				operationId: input.operationId,
				sequence: 1,
				kind: 'delta',
				text: 'Compacted.',
			});
			onFrame({ operationId: input.operationId, sequence: 2, kind: 'done' });
		},
		cancel: async () => true,
	};
	const compaction: AiChatCompaction = {
		threshold: 2,
		summarize: async (input) => {
			expect(input.rehydration.messages).toEqual(original);
			return { summarizesThroughMessageId: 'm3', content: 'Summary.' };
		},
	};
	await prompt(
		controller(persistence, transport, new AiRegistry(), { compaction }),
	).finished;
	expect(messages).toEqual(original);
	expect(events.slice(0, 3)).toEqual([
		'rehydrate',
		'append-context-summary',
		'rehydrate',
	]);
	expect(rehydration?.summaries).toEqual([
		{ summarizesThroughMessageId: 'm3', content: 'Summary.' },
	]);
});

test('invalid provider sequencing finishes the in-progress assistant as failed', async () => {
	const { persistence, writes } = durableHarness();
	let cancelled = 0;
	const result = await prompt(
		controller(persistence, {
			async stream(input, onFrame) {
				onFrame({
					operationId: input.operationId,
					sequence: 2,
					kind: 'delta',
					text: 'late',
				});
			},
			async cancel() {
				cancelled += 1;
				return true;
			},
		}),
	).finished;
	expect(result).toMatchObject({ status: 'failed' });
	expect(writes.at(-1)?.input).toMatchObject({ status: 'failed', content: '' });
	expect(cancelled).toBe(1);
});

test('duplicate provider tool-call IDs fail before an ambiguous durable result is written', async () => {
	const { persistence, writes } = durableHarness();
	const registry = new AiRegistry();
	registry.registerTool(
		{
			name: 'tasks.write',
			description: 'Write a task.',
			inputSchema: { type: 'object' },
			execute: async () => ({ ok: true }),
		},
		{ owner: 'tasks' },
	);
	let cancelled = 0;
	const result = await prompt(
		controller(
			persistence,
			{
				async stream(input, onFrame) {
					onFrame({
						operationId: input.operationId,
						sequence: 1,
						kind: 'tool-call',
						toolCallId: 'call-1',
						toolName: 'tasks.write',
						input: {},
					});
					onFrame({
						operationId: input.operationId,
						sequence: 2,
						kind: 'tool-call',
						toolCallId: 'call-1',
						toolName: 'tasks.write',
						input: {},
					});
				},
				async cancel() {
					cancelled += 1;
					return true;
				},
			},
			registry,
		),
	).finished;

	expect(result).toMatchObject({ status: 'failed' });
	expect(cancelled).toBe(1);
	expect(writes.map((write) => write.method)).not.toContain('begin-tool-call');
	expect(writes.map((write) => write.method)).not.toContain(
		'append-tool-result',
	);
});

test('local cancellation finishes the assistant with safe partial text', async () => {
	const { persistence, writes } = durableHarness();
	let onFrame: ((frame: AiProviderStreamFrame) => void) | undefined;
	let started!: () => void;
	const streaming = new Promise<void>((resolve) => {
		started = resolve;
	});
	let operationId = '';
	const run = prompt(
		controller(persistence, {
			async stream(input, handler) {
				operationId = input.operationId;
				onFrame = handler;
				handler({
					operationId,
					sequence: 1,
					kind: 'delta',
					text: 'Partial\0 response',
				});
				started();
				await new Promise<void>(() => {});
			},
			async cancel() {
				onFrame?.({ operationId, sequence: 2, kind: 'aborted' });
				return true;
			},
		}),
	);
	await streaming;
	run.cancel();
	expect(await run.finished).toMatchObject({
		status: 'cancelled',
		text: 'Partial response',
	});
	expect(writes.at(-1)?.input).toMatchObject({
		status: 'cancelled',
		content: 'Partial response',
	});
	expect(writes.at(-1)?.input).not.toHaveProperty('errorCode');
});

test('cancellation before agent creation does not persist or start a provider stream', async () => {
	const { events, persistence, writes } = durableHarness();
	let release!: () => void;
	const rehydrating = new Promise<void>((resolve) => {
		release = resolve;
	});
	let streams = 0;
	const delayed: AiChatPersistence = {
		...persistence,
		async rehydrate(input) {
			await rehydrating;
			return persistence.rehydrate(input);
		},
	};
	const run = prompt(
		controller(delayed, {
			stream: async () => {
				streams += 1;
			},
			cancel: async () => true,
		}),
	);
	run.cancel();
	release();
	expect(await run.finished).toMatchObject({ status: 'cancelled', text: '' });
	expect(events).toEqual(['rehydrate']);
	expect(writes).toHaveLength(0);
	expect(streams).toBe(0);
});

test('cancellation during a tool does not open the next provider stream', async () => {
	const { persistence, writes } = durableHarness();
	const registry = new AiRegistry();
	let cancel!: () => void;
	registry.registerTool(
		{
			name: 'tasks.write',
			description: 'Write a task.',
			inputSchema: { type: 'object' },
			execute: async () => {
				cancel();
				return { ok: true };
			},
		},
		{ owner: 'tasks' },
	);
	let streams = 0;
	const run = prompt(
		controller(
			persistence,
			{
				async stream(input, onFrame) {
					streams += 1;
					if (streams > 1)
						throw new Error('cancelled runs must not stream again');
					onFrame({
						operationId: input.operationId,
						sequence: 1,
						kind: 'tool-call',
						toolCallId: 'call-1',
						toolName: 'tasks.write',
						input: {},
					});
					onFrame({
						operationId: input.operationId,
						sequence: 2,
						kind: 'done',
					});
				},
				cancel: async () => true,
			},
			registry,
		),
	);
	cancel = () => run.cancel();
	expect(await run.finished).toMatchObject({ status: 'cancelled' });
	expect(streams).toBe(1);
	expect(writes.at(-1)?.input).toMatchObject({ status: 'cancelled' });
});

test('a tool-result persistence failure does not append a duplicate failure result', async () => {
	const { persistence, writes } = durableHarness();
	const registry = new AiRegistry();
	registry.registerTool(
		{
			name: 'tasks.write',
			description: 'Write a task.',
			inputSchema: { type: 'object' },
			execute: async () => ({ ok: true }),
		},
		{ owner: 'tasks' },
	);
	let appendAttempts = 0;
	const failing: AiChatPersistence = {
		...persistence,
		async appendToolResult() {
			appendAttempts += 1;
			throw new Error('disk failure');
		},
	};
	const result = await prompt(
		controller(
			failing,
			{
				async stream(input, onFrame) {
					onFrame({
						operationId: input.operationId,
						sequence: 1,
						kind: 'tool-call',
						toolCallId: 'call-1',
						toolName: 'tasks.write',
						input: {},
					});
					onFrame({
						operationId: input.operationId,
						sequence: 2,
						kind: 'done',
					});
				},
				cancel: async () => true,
			},
			registry,
		),
	).finished;
	expect(result).toMatchObject({ status: 'failed', error: 'disk failure' });
	expect(appendAttempts).toBe(1);
	expect(
		writes.filter((write) => write.method === 'finish-tool-call'),
	).toHaveLength(0);
});

test('retry resumes a canonical user message without duplicating provider history', async () => {
	const resumed: AiChatContextMessage = {
		id: 'user-1',
		kind: 'user',
		status: 'completed',
		content: 'Hello.',
	};
	const prior: AiChatContextMessage = {
		id: 'assistant-0',
		kind: 'assistant',
		status: 'completed',
		content: 'Earlier.',
	};
	const { persistence, writes } = durableHarness({
		messages: [prior, resumed],
	});
	let providerHistory: ReadonlyArray<AiChatContextMessage> = [];
	let currentPrompt: ReadonlyArray<unknown> = [];
	const result = await controller(persistence, {
		async stream(input, onFrame) {
			providerHistory = input.rehydration.messages;
			currentPrompt = input.messages;
			onFrame({ operationId: input.operationId, sequence: 1, kind: 'done' });
		},
		cancel: async () => true,
	}).start({
		chatId: 'chat-1',
		workspaceId: 'workspace-1',
		message: 'Hello.',
		resumeUserMessageId: 'user-1',
	}).finished;
	expect(result.status).toBe('completed');
	expect(providerHistory).toEqual([prior]);
	expect(currentPrompt).toHaveLength(1);
	expect(writes.map((write) => write.method)).not.toContain('append-user');
});

test('retry rejects a user record whose canonical content does not match', async () => {
	const { persistence, writes } = durableHarness({
		messages: [
			{
				id: 'user-1',
				kind: 'user',
				status: 'completed',
				content: 'Different.',
			},
		],
	});
	let streams = 0;
	const result = await controller(persistence, {
		stream: async () => {
			streams += 1;
		},
		cancel: async () => true,
	}).start({
		chatId: 'chat-1',
		workspaceId: 'workspace-1',
		message: 'Hello.',
		resumeUserMessageId: 'user-1',
	}).finished;
	expect(result).toMatchObject({ status: 'failed' });
	expect(writes).toHaveLength(0);
	expect(streams).toBe(0);
});

test('retry rejects a user record that is not completed', async () => {
	const { persistence, writes } = durableHarness({
		messages: [
			{
				id: 'user-1',
				kind: 'user',
				status: 'cancelled',
				content: 'Hello.',
			},
		],
	});
	let streams = 0;
	const result = await controller(persistence, {
		stream: async () => {
			streams += 1;
		},
		cancel: async () => true,
	}).start({
		chatId: 'chat-1',
		workspaceId: 'workspace-1',
		message: 'Hello.',
		resumeUserMessageId: 'user-1',
	}).finished;
	expect(result).toMatchObject({ status: 'failed' });
	expect(writes).toHaveLength(0);
	expect(streams).toBe(0);
});

test('disabling a registered plugin cancels the provider and persists interruption', async () => {
	const { persistence, writes } = durableHarness();
	const registry = new AiRegistry();
	const dispose = registry.registerTool(
		{
			name: 'tasks.write',
			description: 'Write a task.',
			inputSchema: { type: 'object' },
			execute: async () => ({ ok: true }),
		},
		{ owner: 'tasks' },
	);
	let onFrame: ((frame: AiProviderStreamFrame) => void) | undefined;
	let operationId = '';
	let cancelCalls = 0;
	let started!: () => void;
	const streaming = new Promise<void>((resolve) => {
		started = resolve;
	});
	const run = prompt(
		controller(
			persistence,
			{
				async stream(input, handler) {
					operationId = input.operationId;
					onFrame = handler;
					handler({
						operationId,
						sequence: 1,
						kind: 'delta',
						text: 'Partial.',
					});
					started();
					await new Promise<void>(() => {});
				},
				async cancel() {
					cancelCalls += 1;
					onFrame?.({ operationId, sequence: 2, kind: 'aborted' });
					return true;
				},
			},
			registry,
		),
	);
	await streaming;
	dispose();
	expect(await run.finished).toMatchObject({
		status: 'interrupted',
		text: 'Partial.',
	});
	expect(cancelCalls).toBe(1);
	expect(writes.at(-1)?.input).toMatchObject({
		status: 'interrupted',
		content: 'Partial.',
		errorCode: 'plugin-disabled',
	});
});
