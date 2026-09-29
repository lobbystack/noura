import { describe, expect, test } from 'bun:test';
import type {
	Chat,
	ChatMessage,
	ChatRead,
	MutationResult,
} from '@noura/shared';
import {
	CHAT_CHANGED_MESSAGE,
	chatPersistenceError,
	chatTitle,
	createChatPersistence,
	namesChat,
	nextChatRevision,
} from './chat-persistence';
import type { ChatService } from './client';

const chat = {
	id: 'chat_1',
	title: 'New chat',
	revision: 'chat-rev-1',
} as Chat;

function message(fields: Partial<ChatMessage>): ChatMessage {
	return {
		id: 'message_1',
		revision: 'message-rev-1',
		kind: 'user',
		status: 'completed',
		content: '',
		...fields,
	} as ChatMessage;
}

function result<T>(value: T, chatRevision?: string): MutationResult<T> {
	return { value, chatRevision } as MutationResult<T>;
}

function service(overrides: Partial<ChatService>): ChatService {
	const missing = () => {
		throw new Error('unexpected call');
	};
	return new Proxy(overrides, {
		get: (target, key) => target[key as keyof ChatService] ?? missing,
	}) as ChatService;
}

const conflict = {
	code: 'revision_conflict',
	category: 'conflict',
	message: 'The chat changed',
	operation: 'chat_append_user_message',
	retryable: false,
};

describe('chatTitle', () => {
	test('derives a bounded chat title from the first line', () => {
		expect(chatTitle('  Plan the release\nwith risks  ')).toBe(
			'Plan the release',
		);
		expect(chatTitle('   ')).toBe('New chat');
		expect(chatTitle(`${'🙂'.repeat(65)} first message`)).toBe('🙂'.repeat(64));
	});
});

describe('namesChat', () => {
	test('names a new chat that has no user message yet', () => {
		expect(namesChat(chat, [], false)).toBe(true);
		expect(namesChat(chat, [message({ kind: 'assistant' })], false)).toBe(true);
	});

	test('keeps a chosen title, an earlier message, or a resumed run', () => {
		expect(namesChat({ title: 'Release plan' }, [], false)).toBe(false);
		expect(namesChat(chat, [message({ kind: 'user' })], false)).toBe(false);
		expect(namesChat(chat, [], true)).toBe(false);
	});
});

describe('nextChatRevision', () => {
	test('uses the revision the native side reports', () => {
		expect(nextChatRevision({ chatRevision: 'rev-2' })).toBe('rev-2');
	});

	test('fails instead of reading the chat again when it is missing', () => {
		expect(() => nextChatRevision({})).toThrow();
		expect(() => nextChatRevision({ chatRevision: '' })).toThrow();
	});
});

describe('chatPersistenceError', () => {
	test('turns a stale revision into a controller revision conflict', () => {
		const error = chatPersistenceError(conflict, 'chat-rev-1') as Error & {
			code: string;
			expectedRevision: string;
		};
		expect(error.message).toBe(CHAT_CHANGED_MESSAGE);
		expect(error.code).toBe('revision-conflict');
		expect(error.expectedRevision).toBe('chat-rev-1');
	});

	test('passes other errors through', () => {
		const other = new Error('disk full');
		expect(chatPersistenceError(other, 'chat-rev-1')).toBe(other);
	});
});

describe('createChatPersistence', () => {
	const input = {
		chatId: 'chat_1',
		runId: 'run_1',
		content: 'Plan the release\nwith risks',
		expectedChatRevision: 'chat-rev-1',
	};

	test('rehydrates messages and context summaries from the chat file', async () => {
		const read: ChatRead = {
			chat,
			messages: [
				message({ id: 'm1', content: 'Hello' }),
				message({
					id: 'm2',
					kind: 'context-summary',
					content: 'Summary',
					summarizesThroughMessageId: 'm1',
				}),
			],
		} as ChatRead;
		const persistence = createChatPersistence(
			service({ read: async () => read }),
			{ nameFromFirstMessage: false },
		);
		const rehydrated = await persistence.rehydrate({ chatId: 'chat_1' });
		expect(rehydrated.chatRevision).toBe('chat-rev-1');
		expect(rehydrated.messages.map((item) => item.id)).toEqual(['m1', 'm2']);
		expect(rehydrated.summaries).toEqual([
			{ summarizesThroughMessageId: 'm1', content: 'Summary' },
		]);
	});

	test('names the chat from its first message with the returned revision', async () => {
		const renames: unknown[] = [];
		const seen: string[] = [];
		const renamed = { ...chat, title: 'Plan the release', revision: 'rev-3' };
		const persistence = createChatPersistence(
			service({
				appendUserMessage: async () =>
					result(message({ id: 'user_1' }), 'rev-2'),
				rename: async (request) => {
					renames.push(request);
					return result(renamed);
				},
			}),
			{
				nameFromFirstMessage: true,
				onUserMessage: (appended) => seen.push(appended.id),
				onRenamed: (value) => seen.push(value.title),
			},
		);
		expect(await persistence.appendUser(input)).toEqual({
			chatRevision: 'rev-3',
		});
		expect(renames).toEqual([
			{
				chatId: 'chat_1',
				title: 'Plan the release',
				expectedChatRevision: 'rev-2',
			},
		]);
		expect(seen).toEqual(['user_1', 'Plan the release']);
	});

	test('a failed rename does not block the run', async () => {
		const persistence = createChatPersistence(
			service({
				appendUserMessage: async () => result(message({}), 'rev-2'),
				rename: async () => {
					throw conflict;
				},
			}),
			{ nameFromFirstMessage: true },
		);
		expect(await persistence.appendUser(input)).toEqual({
			chatRevision: 'rev-2',
		});
	});

	test('reports a stale revision as a revision conflict', async () => {
		const persistence = createChatPersistence(
			service({
				appendUserMessage: async () => {
					throw conflict;
				},
			}),
			{ nameFromFirstMessage: false },
		);
		await expect(persistence.appendUser(input)).rejects.toMatchObject({
			code: 'revision-conflict',
			expectedRevision: 'chat-rev-1',
		});
	});

	test('sends a null error code when a step finishes without one', async () => {
		const calls: unknown[] = [];
		const persistence = createChatPersistence(
			service({
				finishAssistant: async (request) => {
					calls.push(request);
					return result(message({ kind: 'assistant' }), 'rev-4');
				},
			}),
			{ nameFromFirstMessage: false },
		);
		expect(
			await persistence.finishAssistant({
				chatId: 'chat_1',
				messageId: 'assistant_1',
				content: 'Done',
				status: 'completed',
				expectedChatRevision: 'rev-3',
				expectedMessageRevision: 'message-rev-1',
			}),
		).toEqual({ chatRevision: 'rev-4' });
		expect(calls).toEqual([
			{
				chatId: 'chat_1',
				messageId: 'assistant_1',
				content: 'Done',
				status: 'completed',
				errorCode: null,
				expectedChatRevision: 'rev-3',
				expectedMessageRevision: 'message-rev-1',
			},
		]);
	});
});
