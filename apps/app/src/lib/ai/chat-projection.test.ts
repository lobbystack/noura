import { describe, expect, test } from 'bun:test';
import {
	chatTitle,
	nativeToolDefinitions,
	nativeTransportMessages,
	providerStreamFrame,
	rehydratedTransportMessages,
	transportMessages,
} from './chat-projection';

describe('AI chat projection', () => {
	test('keeps only supported non-empty text messages for native streaming', () => {
		expect(
			transportMessages([
				{ role: 'system', content: [{ type: 'text', text: 'Instructions' }] },
				{ role: 'tool', content: [{ type: 'text', text: 'ignore' }] },
				{ role: 'user', content: [] },
			]),
		).toEqual([
			{ role: 'system', content: [{ type: 'text', text: 'Instructions' }] },
		]);
	});

	test('derives a bounded chat title from the first line', () => {
		expect(chatTitle('  Plan the release\nwith risks  ')).toBe(
			'Plan the release',
		);
		expect(chatTitle('   ')).toBe('New chat');
		expect(chatTitle(`${'🙂'.repeat(65)} first message`)).toBe('🙂'.repeat(64));
	});

	test('round-trips native tool calls and results through provider history', () => {
		expect(
			rehydratedTransportMessages([
				{
					kind: 'tool-call',
					status: 'completed',
					content: '{"path":"notes/plan.md"}',
					toolCallId: 'call_1',
					toolName: 'read_note',
				},
				{
					kind: 'tool-result',
					status: 'completed',
					content: '{"title":"Plan"}',
					toolCallId: 'call_1',
					toolName: 'read_note',
				},
			]),
		).toEqual([
			{
				role: 'assistant',
				content: [
					{
						type: 'toolCall',
						callId: 'call_1',
						name: 'read_note',
						arguments: { path: 'notes/plan.md' },
					},
				],
			},
			{
				role: 'tool',
				content: [
					{
						type: 'toolResult',
						callId: 'call_1',
						name: 'read_note',
						content: '{"title":"Plan"}',
					},
				],
			},
		]);
	});

	test('strips executable plugin code from native tool definitions', () => {
		expect(
			nativeToolDefinitions([
				{
					definition: {
						name: 'read_note',
						description: 'Read a note',
						inputSchema: { type: 'object' },
					},
				},
			]),
		).toEqual([
			{
				name: 'read_note',
				description: 'Read a note',
				inputSchema: { type: 'object' },
			},
		]);
	});

	test('projects native tool-call events for the controller', () => {
		expect(
			providerStreamFrame(
				{
					operationId: 'operation_1',
					event: {
						type: 'toolCall',
						call: {
							callId: 'call_1',
							name: 'read_note',
							arguments: { path: 'notes/plan.md' },
						},
					},
				},
				3,
			),
		).toEqual({
			operationId: 'operation_1',
			sequence: 3,
			kind: 'tool-call',
			toolCallId: 'call_1',
			toolName: 'read_note',
			input: { path: 'notes/plan.md' },
		});
	});

	test('rebuilds provider history from completed canonical messages', () => {
		expect(
			rehydratedTransportMessages([
				{ kind: 'user', status: 'completed', content: 'Earlier question' },
				{ kind: 'assistant', status: 'completed', content: 'Earlier answer' },
				{ kind: 'assistant', status: 'cancelled', content: 'Partial answer' },
				{ kind: 'context-summary', status: 'completed', content: 'Summary' },
			]),
		).toEqual([
			{ role: 'user', content: [{ type: 'text', text: 'Earlier question' }] },
			{
				role: 'assistant',
				content: [{ type: 'text', text: 'Earlier answer' }],
			},
			{ role: 'system', content: [{ type: 'text', text: 'Summary' }] },
		]);
	});

	test('puts a non-empty composed system prompt before canonical and current messages', () => {
		expect(
			nativeTransportMessages(
				'Base rules.\n\nPlugin context.',
				[
					{ kind: 'user', status: 'completed', content: 'Earlier question' },
					{ kind: 'assistant', status: 'completed', content: 'Earlier answer' },
				],
				[
					{
						role: 'user',
						content: [{ type: 'text', text: 'Current question' }],
					},
				],
			),
		).toEqual([
			{
				role: 'system',
				content: [{ type: 'text', text: 'Base rules.\n\nPlugin context.' }],
			},
			{ role: 'user', content: [{ type: 'text', text: 'Earlier question' }] },
			{
				role: 'assistant',
				content: [{ type: 'text', text: 'Earlier answer' }],
			},
			{ role: 'user', content: [{ type: 'text', text: 'Current question' }] },
		]);
		expect(nativeTransportMessages('   ', [], [])).toEqual([]);
	});
});
