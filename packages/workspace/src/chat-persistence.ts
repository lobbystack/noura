import type { AiChatPersistence } from '@noura/ai';
import { isCoreError, type Chat, type ChatMessage } from '@noura/shared';
import type { ChatService } from './client';

/** The title a chat has until its first message names it. */
export const NEW_CHAT_TITLE = 'New chat';

export const CHAT_CHANGED_MESSAGE =
	'This chat changed outside noura. Reload it before trying again.';

/** A chat title from the first line of a message, at most 64 characters. */
export function chatTitle(message: string): string {
	const firstLine = message.trim().split('\n')[0] ?? '';
	return Array.from(firstLine).slice(0, 64).join('') || NEW_CHAT_TITLE;
}

/**
 * Whether the next message should name the chat: it still has the default
 * title, holds no user message yet, and the run is not resuming one.
 */
export function namesChat(
	chat: Pick<Chat, 'title'>,
	messages: ReadonlyArray<Pick<ChatMessage, 'kind'>>,
	resuming: boolean,
): boolean {
	return (
		!resuming &&
		chat.title === NEW_CHAT_TITLE &&
		!messages.some((item) => item.kind === 'user')
	);
}

/**
 * The chat revision a chat message mutation returned. The native side sends
 * it with every message change (`MutationResult.chatRevision`), so the next
 * step of a run can go ahead without reading the chat again.
 */
export function nextChatRevision(result: { chatRevision?: string }): string {
	if (typeof result.chatRevision === 'string' && result.chatRevision)
		return result.chatRevision;
	throw new Error('The chat was saved, but its new revision is missing.');
}

/** Report a stale chat revision in the form the Pi controller expects. */
export function chatPersistenceError(
	error: unknown,
	expectedRevision: string,
): unknown {
	if (!isCoreError(error) || error.code !== 'revision_conflict') return error;
	return Object.assign(new Error(CHAT_CHANGED_MESSAGE), {
		code: 'revision-conflict' as const,
		expectedRevision,
	});
}

export interface ChatPersistenceOptions {
	/** Rename the chat after its first user message. See `namesChat`. */
	nameFromFirstMessage: boolean;
	/** Called once the user message is on disk. */
	onUserMessage?(
		message: ChatMessage,
		input: { chatId: string; runId: string },
	): void;
	/** Called after the first message renamed the chat. */
	onRenamed?(chat: Chat): void;
}

/**
 * Chat persistence for the Pi controller over the chat service. Every step
 * resolves only after its file write commits, and a stale revision becomes a
 * revision conflict the controller does not retry.
 */
export function createChatPersistence(
	chats: ChatService,
	options: ChatPersistenceOptions,
): AiChatPersistence {
	return {
		rehydrate: async ({ chatId }) => {
			const canonical = await chats.read(chatId);
			return {
				chatRevision: canonical.chat.revision,
				messages: canonical.messages.map((item) => ({
					id: item.id,
					kind: item.kind,
					status: item.status,
					content: item.content,
					toolCallId: item.toolCallId,
					toolName: item.toolName,
				})),
				summaries: canonical.messages.flatMap((item) =>
					item.kind === 'context-summary' && item.summarizesThroughMessageId
						? [
								{
									summarizesThroughMessageId: item.summarizesThroughMessageId,
									content: item.content,
								},
							]
						: [],
				),
			};
		},
		appendUser: async (input) => {
			try {
				const appended = await chats.appendUserMessage(input);
				options.onUserMessage?.(appended.value, input);
				const revision = nextChatRevision(appended);
				// The first message names a new chat. The expected revision
				// makes any change from elsewhere win over the derived title.
				if (options.nameFromFirstMessage) {
					try {
						const renamed = await chats.rename({
							chatId: input.chatId,
							title: chatTitle(input.content),
							expectedChatRevision: revision,
						});
						options.onRenamed?.(renamed.value);
						return { chatRevision: renamed.value.revision };
					} catch {
						// Title derivation must not block a run.
					}
				}
				return { chatRevision: revision };
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		beginAssistant: async (input) => {
			try {
				const result = await chats.beginAssistant(input);
				return {
					chatRevision: nextChatRevision(result),
					message: { id: result.value.id, revision: result.value.revision },
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		beginToolCall: async (input) => {
			try {
				const result = await chats.beginToolCall(input);
				return {
					chatRevision: nextChatRevision(result),
					message: { id: result.value.id, revision: result.value.revision },
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		appendToolResult: async (input) => {
			try {
				const result = await chats.appendToolResult(input);
				return {
					chatRevision: nextChatRevision(result),
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		finishToolCall: async (input) => {
			try {
				const result = await chats.finishToolCall({
					...input,
					errorCode: input.errorCode ?? null,
				});
				return {
					chatRevision: nextChatRevision(result),
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		finishAssistant: async (input) => {
			try {
				const result = await chats.finishAssistant({
					...input,
					errorCode: input.errorCode ?? null,
				});
				return {
					chatRevision: nextChatRevision(result),
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
		appendContextSummary: async (input) => {
			try {
				const result = await chats.appendContextSummary(input);
				return {
					chatRevision: nextChatRevision(result),
					message: { id: result.value.id, revision: result.value.revision },
				};
			} catch (error) {
				throw chatPersistenceError(error, input.expectedChatRevision);
			}
		},
	};
}
