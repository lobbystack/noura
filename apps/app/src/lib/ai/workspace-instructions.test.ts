import { describe, expect, test } from 'bun:test';
import {
	NOURA_BASE_INSTRUCTIONS,
	workspaceInstructions,
} from './workspace-instructions';

describe('workspace AI instructions', () => {
	test('adds a non-empty workspace AGENTS.md after the base instructions', async () => {
		const client = {
			files: {
				readRawMarkdown: async () => ({ body: 'Keep tasks concise.' }),
			},
		} as never;
		expect(await workspaceInstructions(client)).toBe(
			'## Workspace instructions: AGENTS.md\nKeep tasks concise.',
		);
		expect(NOURA_BASE_INSTRUCTIONS).toContain(
			'local-first workspace assistant',
		);
	});

	test('treats an unavailable AGENTS.md as optional', async () => {
		const client = {
			files: {
				readRawMarkdown: async () => Promise.reject(new Error('missing')),
			},
		} as never;
		expect(await workspaceInstructions(client)).toBe('');
	});
});
