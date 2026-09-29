import {
	CHAT_CHANGED_MESSAGE,
	createChatPersistence,
	isCoreError,
	namesChat,
	NEW_CHAT_TITLE,
	type AiProviderConfig,
	type Chat,
	type ChatRead,
} from '@noura/workspace';
import type {
	AiChatRun,
	AiToolConsentRequest,
	PiChatControllerOptions,
} from '@noura/ai';
import { getNouraClient, workspace } from '$lib/state.svelte';
import { LiveRefresh } from '$lib/live-refresh';
import {
	nativeToolDefinitions,
	nativeTransportMessages,
	providerStreamFrame,
} from './chat-projection';
import { AI_POLICY_VERSION } from './policy';
import { composedBaseInstructions } from './workspace-instructions';
import { plugins } from '$lib/plugins.svelte';

function errorMessage(error: unknown) {
	if (error instanceof Error) return error.message;
	if (error && typeof error === 'object' && 'message' in error)
		return String(error.message);
	return 'The AI request could not be completed.';
}

function operationId() {
	return crypto.randomUUID();
}

interface PendingToolApproval {
	workspaceId: string;
	chatId: string;
	runId: string;
	toolCallId: string;
	tool: AiToolConsentRequest['tool'];
	input: unknown;
}

function chatIdFromEvent(payload: unknown): string | null {
	if (!payload || typeof payload !== 'object' || !('id' in payload))
		return null;
	return typeof payload.id === 'string' ? payload.id : null;
}

/**
 * Context and output limits for a provider. Provider settings do not store
 * them yet, so these conservative defaults apply unless a provider entry
 * carries its own values.
 */
function modelLimits(provider: AiProviderConfig) {
	const configured = provider as AiProviderConfig & {
		contextWindow?: unknown;
		maxTokens?: unknown;
	};
	const positive = (value: unknown, fallback: number) =>
		typeof value === 'number' && Number.isFinite(value) && value > 0
			? value
			: fallback;
	return {
		contextWindow: positive(configured.contextWindow, 8_192),
		maxTokens: positive(configured.maxTokens, 1_024),
	};
}

class AiChatStore {
	providers = $state<AiProviderConfig[]>([]);
	chats = $state<Chat[]>([]);
	read = $state<ChatRead | null>(null);
	selectedProviderId = $state('');
	streamingText = $state('');
	running = $state(false);
	changingRetention = $state(false);
	loading = $state(false);
	error = $state<string | null>(null);
	conflict = $state(false);
	pendingToolApproval = $state<PendingToolApproval | null>(null);

	#initialized = false;
	#initializing: Promise<void> | null = null;
	#refreshSequence = 0;
	#chatListSequence = 0;
	/** Chat events arrive once per persisted step of a run. Read the list
	 * once per burst, not once per event. */
	#chatEvents = new LiveRefresh({
		refresh: () => this.#refreshForChatEvents(),
	});
	#changedChatIds = new Set<string>();
	#workspaceId: string | null = null;
	#run: AiChatRun | null = null;
	#resolveToolApproval: ((approved: boolean) => void) | null = null;
	#lastMessage: {
		workspaceId: string;
		chatId: string;
		content: string;
		runId: string;
		userMessageId?: string;
	} | null = null;

	get readyProviders() {
		return this.providers.filter((provider) =>
			Boolean(provider.enabled && provider.credentialRef),
		);
	}

	get selectedProvider() {
		return (
			this.readyProviders.find(
				(provider) => provider.id === this.selectedProviderId,
			) ??
			this.readyProviders[0] ??
			null
		);
	}

	get selectedChatId() {
		return this.read?.chat.id ?? null;
	}

	/** Load providers and chats, then follow chat and workspace events. The
	 * AI page and AI settings call this; nothing runs at app startup. */
	async init() {
		if (this.#initialized) return;
		if (this.#initializing) return this.#initializing;
		this.#initializing = (async () => {
			await this.refresh();
			try {
				await getNouraClient().events.subscribe((event) => {
					if (
						event.type === 'workspace:ready' ||
						event.type === 'workspace:closed'
					)
						void this.refresh();
					if (
						event.type.startsWith('chat:') &&
						event.workspaceId === this.#currentWorkspaceId()
					) {
						const chatId = chatIdFromEvent(event.payload);
						if (chatId) this.#changedChatIds.add(chatId);
						this.#chatEvents.invalidate();
					}
				});
				this.#initialized = true;
			} catch (error) {
				// Leave initialization retryable when the host event bridge is unavailable.
				this.error = errorMessage(error);
			}
		})().finally(() => {
			this.#initializing = null;
		});
		return this.#initializing;
	}

	async refresh() {
		const refreshSequence = ++this.#refreshSequence;
		const workspaceId = this.#currentWorkspaceId();
		this.#reconcileWorkspace(workspaceId);
		this.loading = true;
		this.error = null;
		const client = getNouraClient();
		const [providerResult, chatResult] = await Promise.all([
			Promise.resolve()
				.then(() => client.ai.listProviders())
				.then(
					(value) => ({ value }),
					(error) => ({ error }),
				),
			workspaceId
				? Promise.resolve()
						.then(() => client.chats.list())
						.then(
							(value) => ({ value }),
							(error) => ({ error }),
						)
				: Promise.resolve({ value: [] as Chat[] }),
		]);
		if (refreshSequence !== this.#refreshSequence) return;

		if ('value' in providerResult) {
			const providers = providerResult.value;
			this.providers = providers;
			if (
				!this.selectedProviderId ||
				!providers.some((p) => p.id === this.selectedProviderId)
			)
				this.selectedProviderId =
					providers.find(
						(provider) => provider.enabled && provider.credentialRef,
					)?.id ?? '';
		}

		if (!this.#isCurrentWorkspace(workspaceId)) {
			this.#reconcileWorkspace(this.#currentWorkspaceId());
		} else if ('value' in chatResult) {
			const chats = chatResult.value;
			this.chats = chats;
			if (this.read && !chats.some((chat) => chat.id === this.read?.chat.id))
				this.read = null;
		} else if (
			isCoreError(chatResult.error) &&
			chatResult.error.code === 'workspace_not_open'
		) {
			this.#clearWorkspaceProjection();
		} else {
			this.error = errorMessage(chatResult.error);
		}

		if ('error' in providerResult)
			this.error = errorMessage(providerResult.error);
		this.loading = false;
	}

	/** Re-list chats after a burst of chat events, and re-read the open
	 * chat when something else changed it. A running reply keeps its own
	 * transcript and re-reads once it finishes. */
	async #refreshForChatEvents() {
		const changed = new Set(this.#changedChatIds);
		this.#changedChatIds.clear();
		const workspaceId = this.#currentWorkspaceId();
		if (!workspaceId) return;
		const sequence = ++this.#chatListSequence;
		const client = getNouraClient();
		try {
			const chats = await client.chats.list();
			if (
				sequence !== this.#chatListSequence ||
				!this.#isCurrentWorkspace(workspaceId)
			)
				return;
			this.chats = chats;
			if (this.read && !chats.some((chat) => chat.id === this.read?.chat.id))
				this.read = null;
			const selected = this.selectedChatId;
			if (!selected || this.running || !changed.has(selected)) return;
			const listed = chats.find((chat) => chat.id === selected);
			if (listed?.revision === this.read?.chat.revision) return;
			const read = await client.chats.read(selected);
			if (this.#isCurrentWorkspace(workspaceId) && !this.running)
				this.read = read;
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
		}
	}

	#currentWorkspaceId() {
		const state = workspace.state;
		return state?.phase === 'ready' ? (state.workspaceId ?? null) : null;
	}

	#isCurrentWorkspace(workspaceId: string | null) {
		return this.#currentWorkspaceId() === workspaceId;
	}

	#reconcileWorkspace(workspaceId: string | null) {
		if (this.#workspaceId === workspaceId) return;
		this.#workspaceId = workspaceId;
		this.#clearActiveRun();
		this.#lastMessage = null;
		this.chats = [];
		this.read = null;
		this.conflict = false;
	}

	#clearWorkspaceProjection() {
		this.#workspaceId = null;
		this.#clearActiveRun();
		this.#lastMessage = null;
		this.chats = [];
		this.read = null;
		this.conflict = false;
	}

	#clearActiveRun() {
		this.#resolvePendingToolApproval(false);
		this.#run?.cancel();
		this.#run = null;
		this.running = false;
		this.streamingText = '';
	}

	#resolvePendingToolApproval(approved: boolean) {
		const resolve = this.#resolveToolApproval;
		this.#resolveToolApproval = null;
		this.pendingToolApproval = null;
		resolve?.(approved);
	}

	#requestToolApproval(
		workspaceId: string,
		request: AiToolConsentRequest,
	): Promise<boolean> {
		if (!this.#isCurrentWorkspace(workspaceId)) return Promise.resolve(false);
		this.#resolvePendingToolApproval(false);
		return new Promise((resolve) => {
			this.#resolveToolApproval = resolve;
			this.pendingToolApproval = {
				workspaceId,
				chatId: request.chatId,
				runId: request.runId,
				toolCallId: request.toolCallId,
				tool: request.tool,
				input: request.input,
			};
		});
	}

	allowToolOnce() {
		this.#resolvePendingToolApproval(true);
	}

	denyToolUse() {
		this.#resolvePendingToolApproval(false);
	}

	async select(chatId: string) {
		if (this.running) return;
		const workspaceId = this.#currentWorkspaceId();
		if (!workspaceId) return;
		this.loading = true;
		this.error = null;
		this.conflict = false;
		try {
			const read = await getNouraClient().chats.recoverInterrupted(chatId);
			if (!this.#isCurrentWorkspace(workspaceId)) return;
			this.read = read;
			this.streamingText = '';
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
		} finally {
			if (this.#isCurrentWorkspace(workspaceId)) this.loading = false;
		}
	}

	async create(retention: Chat['retention']) {
		const workspaceId = this.#currentWorkspaceId();
		if (!workspaceId || this.running) return;
		this.error = null;
		try {
			const result = await getNouraClient().chats.create({
				title: NEW_CHAT_TITLE,
				retention,
			});
			if (!this.#isCurrentWorkspace(workspaceId)) return;
			this.chats = [
				result.value,
				...this.chats.filter((chat) => chat.id !== result.value.id),
			];
			await this.select(result.value.id);
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
		}
	}

	async changeRetention(retention: Chat['retention']) {
		const chat = this.read?.chat;
		const workspaceId = this.#currentWorkspaceId();
		if (!chat || !workspaceId || this.running || this.changingRetention) return;
		this.changingRetention = true;
		this.error = null;
		try {
			const result = await getNouraClient().chats.changeRetention({
				chatId: chat.id,
				retention,
				retentionDays: retention === 'ephemeral' ? 30 : null,
				expectedChatRevision: chat.revision,
			});
			if (!this.#isCurrentWorkspace(workspaceId)) return;
			this.#adoptChat(result.value);
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
		} finally {
			this.changingRetention = false;
		}
	}

	/** Rename a chat. Blank titles are ignored. */
	async rename(chatId: string, title: string) {
		const workspaceId = this.#currentWorkspaceId();
		const chat =
			this.chats.find((entry) => entry.id === chatId) ??
			(this.read?.chat.id === chatId ? this.read.chat : undefined);
		const next = title.trim();
		if (!workspaceId || !chat || !next || next === chat.title) return;
		this.error = null;
		try {
			const result = await getNouraClient().chats.rename({
				chatId,
				title: next,
				expectedChatRevision: chat.revision,
			});
			if (this.#isCurrentWorkspace(workspaceId)) this.#adoptChat(result.value);
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error =
					isCoreError(error) && error.code === 'revision_conflict'
						? CHAT_CHANGED_MESSAGE
						: errorMessage(error);
		}
	}

	#adoptChat(chat: Chat) {
		this.chats = this.chats.map((entry) =>
			entry.id === chat.id ? chat : entry,
		);
		if (this.read?.chat.id === chat.id) this.read = { ...this.read, chat };
	}

	async send(message: string, resumeUserMessageId?: string) {
		const chat = this.read?.chat;
		const provider = this.selectedProvider;
		const workspaceId = this.#currentWorkspaceId();
		if (this.running) return;
		if (!workspaceId) {
			this.error = 'Open a workspace before sending an AI message.';
			return;
		}
		if (!chat || !provider) return;

		this.error = null;
		this.conflict = false;
		this.streamingText = '';
		this.running = true;
		const client = getNouraClient();
		// A run snapshots currently enabled plugin contributions before composing
		// its prompt; the controller cancels if that registry changes mid-run.
		try {
			await plugins.sync();
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
			this.running = false;
			return;
		}
		if (!this.#isCurrentWorkspace(workspaceId)) {
			this.running = false;
			return;
		}
		const systemPrompt = await composedBaseInstructions(client);
		if (!this.#isCurrentWorkspace(workspaceId)) {
			this.running = false;
			return;
		}
		const runId = `run_${operationId()}`;
		const nameFirstMessage = namesChat(
			chat,
			this.read?.messages ?? [],
			Boolean(resumeUserMessageId),
		);
		this.#lastMessage = {
			workspaceId,
			chatId: chat.id,
			content: message,
			runId,
			...(resumeUserMessageId ? { userMessageId: resumeUserMessageId } : {}),
		};
		let persistedUserMessageId = resumeUserMessageId;

		const model = {
			id: provider.model,
			name: provider.model,
			api: 'noura-native',
			provider: provider.id,
			baseUrl: provider.endpoint ?? '',
			reasoning: false,
			input: ['text'],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			...modelLimits(provider),
		} as PiChatControllerOptions['model'];

		let PiChatController: typeof import('./chat-controller').PiChatController;
		try {
			({ PiChatController } = await import('./chat-controller'));
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
			this.running = false;
			return;
		}
		if (!this.#isCurrentWorkspace(workspaceId)) {
			this.running = false;
			return;
		}
		const controller = new PiChatController({
			model,
			systemPrompt,
			registry: client.ai.registry,
			runId: () => runId,
			consent: {
				policy: async () => 'prompt',
				requestToolUse: (request) =>
					this.#requestToolApproval(workspaceId, request),
			},
			persistence: createChatPersistence(client.chats, {
				nameFromFirstMessage: nameFirstMessage,
				onUserMessage: (appended, input) => {
					persistedUserMessageId = appended.id;
					if (
						this.#lastMessage?.runId === input.runId &&
						this.#lastMessage.chatId === input.chatId
					)
						this.#lastMessage.userMessageId = appended.id;
				},
				onRenamed: (renamed) => {
					if (this.#isCurrentWorkspace(workspaceId)) this.#adoptChat(renamed);
				},
			}),
			providerTransport: {
				stream: async (input, onFrame) => {
					let nextSequence = 1;
					let expectedNativeSequence = 1;
					let streamError: string | null = null;
					await client.ai.stream(
						{
							operationId: input.operationId,
							model: { providerId: provider.id, model: provider.model },
							messages: nativeTransportMessages(
								input.systemPrompt,
								input.rehydration.messages,
								input.messages,
							),
							tools: nativeToolDefinitions(client.ai.registry.toolEntries()),
							policyVersion: AI_POLICY_VERSION,
						},
						(frame) => {
							if (Number(frame.sequence) !== expectedNativeSequence) {
								streamError =
									'The native AI stream delivered an invalid frame sequence.';
								return;
							}
							expectedNativeSequence += 1;
							// Native streams include `started`; the controller starts at the
							// first delta or terminal frame, so sequence numbers are projected.
							if (frame.event.type === 'started') return;
							if (frame.event.type === 'error') {
								streamError = frame.event.error.message;
								return;
							}
							if (
								frame.event.type === 'textDelta' &&
								this.#isCurrentWorkspace(workspaceId)
							)
								this.streamingText += frame.event.text;
							const projected = providerStreamFrame(frame, nextSequence);
							if (projected) {
								nextSequence += 1;
								onFrame(projected);
							}
						},
					);
					if (streamError) throw new Error(streamError);
				},
				cancel: async (id) => (await client.ai.cancel(id)).cancelled,
			},
		});

		try {
			const run = controller.start({
				chatId: chat.id,
				message,
				workspaceId,
				providerId: provider.id,
				...(persistedUserMessageId
					? { resumeUserMessageId: persistedUserMessageId }
					: {}),
			});
			this.#run = run;
			const result = await run.finished;
			if (!this.#isCurrentWorkspace(workspaceId)) return;
			const read = await client.chats.read(chat.id);
			if (!this.#isCurrentWorkspace(workspaceId)) return;
			this.read = read;
			this.#adoptChat(read.chat);
			if (result.status === 'completed') {
				if (this.#lastMessage?.runId === result.runId) this.#lastMessage = null;
			} else if (result.conflict) {
				this.conflict = true;
				this.error = CHAT_CHANGED_MESSAGE;
			} else if (result.status !== 'cancelled') {
				this.error = result.error ?? 'The reply did not finish.';
			}
		} catch (error) {
			if (this.#isCurrentWorkspace(workspaceId))
				this.error = errorMessage(error);
		} finally {
			if (this.#run?.runId === runId) {
				this.#run = null;
				this.running = false;
				this.streamingText = '';
			}
		}
	}

	async retry() {
		const lastMessage = this.#lastMessage;
		if (
			lastMessage?.workspaceId === this.#currentWorkspaceId() &&
			lastMessage.chatId === this.selectedChatId
		) {
			let userMessageId = lastMessage.userMessageId;
			if (!userMessageId) {
				try {
					const canonical = await getNouraClient().chats.read(
						lastMessage.chatId,
					);
					userMessageId = canonical.messages.find(
						(message) =>
							message.kind === 'user' &&
							message.status === 'completed' &&
							message.runId === lastMessage.runId &&
							message.content === lastMessage.content,
					)?.id;
				} catch (error) {
					this.error = errorMessage(error);
					return;
				}
			}
			await this.send(lastMessage.content, userMessageId);
			return;
		}
		await this.init();
		await this.refresh();
	}

	stop() {
		this.#resolvePendingToolApproval(false);
		this.#run?.cancel();
	}
}

export const aiChats = new AiChatStore();
