import { describe, expect, test } from 'bun:test';
import {
	createNouraClient,
	firstPartyPlugins,
	type CoreTransport,
	type Task,
	type UnmanagedFile,
	type WorkspaceEntry,
} from './index';

function transport(responses: Record<string, unknown>): CoreTransport {
	const calls: Array<{ command: string; payload?: Record<string, unknown> }> =
		[];
	return {
		calls,
		request: async <T>(command: string, payload?: Record<string, unknown>) => {
			calls.push(payload === undefined ? { command } : { command, payload });
			return responses[command] as T;
		},
		stream: async <T>(
			command: string,
			payload: Record<string, unknown>,
			handler: (frame: T) => void,
		) => {
			calls.push({ command, payload });
			for (const frame of (responses[command] as T[] | undefined) ?? [])
				handler(frame);
		},
		subscribe: async () => () => {},
	} as CoreTransport & { calls: typeof calls };
}

describe('typed client', () => {
	test('registers every Initial MVP domain as a first-party plugin', () => {
		expect(firstPartyPlugins.map((plugin) => plugin.manifest.id)).toEqual([
			'ai',
			'folders',
			'notes',
			'tasks',
			'calendar',
			'projects',
		]);
	});
	test('delegates native workspace folder selection through the typed boundary', async () => {
		const mock = transport({ workspace_pick_folder: '/Users/example/Notes' });
		const client = createNouraClient(mock);

		await expect(
			client.workspaces.pickFolder({ title: 'Open a Noura workspace' }),
		).resolves.toBe('/Users/example/Notes');
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({
			command: 'workspace_pick_folder',
			payload: { title: 'Open a Noura workspace' },
		});
	});
	test('delegates AI streaming and cancellation through the typed boundary', async () => {
		const operationId = '7cd5ab0c-b143-4ae7-9e99-867e20b8cd73';
		const frames = [
			{
				operationId,
				sequence: 1,
				event: {
					type: 'started',
					model: { providerId: 'test', model: 'test-model' },
				},
			},
		];
		const mock = transport({
			ai_stream: frames,
			ai_stream_cancel: { operationId, cancelled: true },
		});
		const client = createNouraClient(mock);
		const received: unknown[] = [];

		await client.ai.stream(
			{
				operationId,
				model: { providerId: 'test', model: 'test-model' },
				messages: [
					{ role: 'user', content: [{ type: 'text', text: 'Hello' }] },
				],
				tools: [],
			},
			(frame) => received.push(frame),
		);
		await expect(client.ai.cancel(operationId)).resolves.toEqual({
			operationId,
			cancelled: true,
		});
		expect(received).toEqual(frames);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{
				command: 'ai_stream',
				payload: {
					input: {
						operationId,
						model: { providerId: 'test', model: 'test-model' },
						messages: [
							{ role: 'user', content: [{ type: 'text', text: 'Hello' }] },
						],
						tools: [],
					},
				},
			},
			{ command: 'ai_stream_cancel', payload: { operationId } },
		]);
	});
	test('delegates device-local AI consent without a workspace ID', async () => {
		const mock = transport({
			ai_consent_read: null,
			ai_consent_grant: {
				providerId: 'test',
				policyVersion: '2026-09',
				dataCategory: 'workspace-content',
				grantedAt: '2026-09-03T00:00:00Z',
			},
			ai_consent_revoke: {
				providerId: 'test',
				policyVersion: '2026-09',
				revoked: true,
				cancelledOperations: 1,
			},
		});
		const client = createNouraClient(mock);
		const input = { providerId: 'test', policyVersion: '2026-09' };

		await expect(client.ai.readConsent(input)).resolves.toBeNull();
		await client.ai.grantConsent({
			...input,
			dataCategory: 'workspace-content',
		});
		await client.ai.revokeConsent(input);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{ command: 'ai_consent_read', payload: { input } },
			{
				command: 'ai_consent_grant',
				payload: { input: { ...input, dataCategory: 'workspace-content' } },
			},
			{ command: 'ai_consent_revoke', payload: { input } },
		]);
	});
	test('delegates durable chat lifecycle commands through the typed boundary', async () => {
		const mock = transport({
			chats_create: { value: {}, revision: 'chat-1' },
			chats_change_retention: { value: {}, revision: 'chat-2' },
			chats_rename: { value: {}, revision: 'chat-3' },
			chats_append_user_message: { value: {}, revision: 'message-1' },
			chats_finish_assistant: { value: {}, revision: 'message-2' },
			chats_finish_tool_call: { value: {}, revision: 'message-3' },
			chats_recover_interrupted: { chat: {}, messages: [] },
			chats_expire: ['chat_old'],
		});
		const client = createNouraClient(mock);

		await client.chats.create({ title: 'Planning' });
		await client.chats.changeRetention({
			chatId: 'chat_01j00000000000000000000000',
			retention: 'ephemeral',
			retentionDays: 30,
			expectedChatRevision: 'chat-1',
		});
		await client.chats.rename({
			chatId: 'chat_01j00000000000000000000000',
			title: 'Release planning',
			expectedChatRevision: 'chat-2',
		});
		await client.chats.appendUserMessage({
			chatId: 'chat_01j00000000000000000000000',
			runId: 'run_1',
			content: 'Hello',
			expectedChatRevision: 'chat-1',
		});
		await client.chats.finishAssistant({
			chatId: 'chat_01j00000000000000000000000',
			messageId: 'chat-message_01j00000000000000000000000',
			content: 'Partial response',
			status: 'cancelled',
			errorCode: null,
			expectedChatRevision: 'chat-1',
			expectedMessageRevision: 'message-1',
		});
		await client.chats.finishToolCall({
			chatId: 'chat_01j00000000000000000000000',
			messageId: 'chat-message_01j00000000000000000000000',
			content: '{"error":"provider unavailable"}',
			status: 'failed',
			errorCode: 'provider_unavailable',
			expectedChatRevision: 'chat-1',
			expectedMessageRevision: 'message-1',
		});
		await client.chats.recoverInterrupted('chat_01j00000000000000000000000');
		await client.chats.expire('2031-01-01T00:00:00Z');

		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{
				command: 'chats_create',
				payload: {
					input: {
						title: 'Planning',
						retention: 'permanent',
						retentionDays: null,
					},
				},
			},
			{
				command: 'chats_change_retention',
				payload: {
					input: {
						chatId: 'chat_01j00000000000000000000000',
						retention: 'ephemeral',
						retentionDays: 30,
						expectedChatRevision: 'chat-1',
					},
				},
			},
			{
				command: 'chats_rename',
				payload: {
					input: {
						chatId: 'chat_01j00000000000000000000000',
						title: 'Release planning',
						expectedChatRevision: 'chat-2',
					},
				},
			},
			{
				command: 'chats_append_user_message',
				payload: {
					input: {
						chatId: 'chat_01j00000000000000000000000',
						runId: 'run_1',
						content: 'Hello',
						expectedChatRevision: 'chat-1',
					},
				},
			},
			{
				command: 'chats_finish_assistant',
				payload: {
					input: {
						chatId: 'chat_01j00000000000000000000000',
						messageId: 'chat-message_01j00000000000000000000000',
						content: 'Partial response',
						status: 'cancelled',
						errorCode: null,
						expectedChatRevision: 'chat-1',
						expectedMessageRevision: 'message-1',
					},
				},
			},
			{
				command: 'chats_finish_tool_call',
				payload: {
					input: {
						chatId: 'chat_01j00000000000000000000000',
						messageId: 'chat-message_01j00000000000000000000000',
						content: '{"error":"provider unavailable"}',
						status: 'failed',
						errorCode: 'provider_unavailable',
						expectedChatRevision: 'chat-1',
						expectedMessageRevision: 'message-1',
					},
				},
			},
			{
				command: 'chats_recover_interrupted',
				payload: { id: 'chat_01j00000000000000000000000' },
			},
			{
				command: 'chats_expire',
				payload: { now: '2031-01-01T00:00:00Z' },
			},
		]);
	});
	test('delegates workspace file discovery through the typed boundary', async () => {
		const entries: WorkspaceEntry[] = [
			{
				relativePath: 'notes',
				name: 'notes',
				kind: 'folder',
				parseStatus: null,
				objectId: null,
				objectType: null,
				revision: null,
			},
		];
		const mock = transport({ files_list: entries });
		const client = createNouraClient(mock);

		await expect(client.files.list()).resolves.toEqual(entries);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({ command: 'files_list' });
	});
	test('delegates non-managed Markdown discovery through the typed boundary', async () => {
		const files: UnmanagedFile[] = [
			{
				relativePath: 'draft.md',
				title: 'Draft',
				body: '',
				revision: 'abc',
				parseStatus: 'unmanaged',
				parseError: null,
			},
		];
		const mock = transport({ files_list_non_managed_markdown: files });
		const client = createNouraClient(mock);

		await expect(client.files.listNonManagedMarkdown()).resolves.toEqual(files);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({ command: 'files_list_non_managed_markdown' });
	});
	test('passes a raw Markdown path as the command payload', async () => {
		const document = {
			relativePath: 'draft.md',
			body: 'draft',
			revision: 'abc',
			usesCrlf: false,
			hasBom: false,
		};
		const mock = transport({ raw_markdown_read: document });
		const client = createNouraClient(mock);

		await expect(
			client.files.readRawMarkdown({ relativePath: 'draft.md' }),
		).resolves.toEqual(document);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({
			command: 'raw_markdown_read',
			payload: { relativePath: 'draft.md' },
		});
	});
	test('resolves Markdown links and assets relative to the source document', async () => {
		const mock = transport({
			files_resolve_markdown_link: { kind: 'unresolved' },
			files_read_local_asset: { dataUrl: 'data:image/png;base64,AA==' },
		});
		const client = createNouraClient(mock);
		await client.files.resolveMarkdownLink({
			sourceRelativePath: 'notes/a.md',
			target: '../image.png',
		});
		await client.files.readLocalAsset({
			sourceRelativePath: 'notes/a.md',
			target: '../image.png',
		});
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{
				command: 'files_resolve_markdown_link',
				payload: {
					input: { sourceRelativePath: 'notes/a.md', target: '../image.png' },
				},
			},
			{
				command: 'files_read_local_asset',
				payload: {
					input: { sourceRelativePath: 'notes/a.md', target: '../image.png' },
				},
			},
		]);
	});
	test('task completion delegates to generic revision-checked object update', async () => {
		const mock = transport({
			objects_update: {
				value: {},
				revision: 'next',
				durability: 'committed',
				indexStatus: 'updated',
				warnings: [],
			},
		});
		const client = createNouraClient(mock);
		await client.tasks.complete({ id: 'task_01k', expectedRevision: 'old' });
		expect(
			(
				mock as CoreTransport & {
					calls: Array<{ command: string; payload?: Record<string, unknown> }>;
				}
			).calls[0],
		).toEqual({
			command: 'objects_update',
			payload: {
				id: 'task_01k',
				patch: { expectedRevision: 'old', properties: { status: 'done' } },
			},
		});
	});
	test('delegates draft reconciliation and explicit resolution through typed note commands', async () => {
		const mock = transport({
			notes_reconcile_draft: { status: 'conflict', current: {}, body: null },
			notes_resolve_conflict: { value: {}, revision: 'next' },
		});
		const client = createNouraClient(mock);
		await client.notes.reconcileDraft({
			id: 'note_01k',
			baseRevision: 'base',
			baseBody: 'before',
			localBody: 'local',
		});
		await client.notes.resolveConflict({
			id: 'note_01k',
			currentRevision: 'current',
			localBody: 'local',
			resolution: 'replace-external',
		});
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{
				command: 'notes_reconcile_draft',
				payload: {
					input: {
						id: 'note_01k',
						baseRevision: 'base',
						baseBody: 'before',
						localBody: 'local',
					},
				},
			},
			{
				command: 'notes_resolve_conflict',
				payload: {
					input: {
						id: 'note_01k',
						currentRevision: 'current',
						localBody: 'local',
						resolution: 'replace-external',
					},
				},
			},
		]);
	});
	test('kanban projection groups and orders indexed tasks without board storage', async () => {
		const tasks = [
			{
				id: 'task_b',
				type: 'task',
				title: 'B',
				body: '',
				relativePath: 'b.md',
				revision: '1',
				properties: { status: 'todo', priority: 'medium', kanban_order: 'b' },
			},
			{
				id: 'task_a',
				type: 'task',
				title: 'A',
				body: '',
				relativePath: 'a.md',
				revision: '1',
				properties: { status: 'todo', priority: 'medium', kanban_order: 'a' },
			},
		] as Task[];
		const client = createNouraClient(transport({ objects_query: tasks }));
		const board = await client.kanban.getBoard();
		expect(board.groups[0]?.items.map((task) => task.id)).toEqual([
			'task_a',
			'task_b',
		]);
	});
});

test('PDF reads and external clicks use the public file service', async () => {
	const data = {
		relativePath: 'lecture.pdf',
		revision: 'r1',
		bytes: new Uint8Array([37, 80, 68, 70]),
	};
	const core = transport({ files_read_pdf: data });
	const client = createNouraClient(core);
	expect(await client.files.readPdf({ relativePath: 'lecture.pdf' })).toBe(
		data,
	);
	await client.files.openPdfLink('https://example.com');
	expect((core as CoreTransport & { calls: unknown[] }).calls).toEqual([
		{ command: 'files_read_pdf', payload: { relativePath: 'lecture.pdf' } },
		{ command: 'files_open_pdf_link', payload: { url: 'https://example.com' } },
	]);
});
