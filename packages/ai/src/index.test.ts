import { expect, test } from 'bun:test';
import { AiRegistry } from './index';

test('AI tool registration rejects duplicate public names', () => {
	const registry = new AiRegistry();
	registry.registerTool({
		name: 'workspace.search',
		description: 'Search',
		inputSchema: {},
		execute: async () => [],
	});
	expect(() =>
		registry.registerTool({
			name: 'workspace.search',
			description: 'Duplicate',
			inputSchema: {},
			execute: async () => [],
		}),
	).toThrow();
});

test('AI registry tracks contribution ownership, revision, and risk', () => {
	const registry = new AiRegistry();
	const dispose = registry.registerInstructionProvider(
		{
			id: 'calendar.rules',
			risk: 'medium',
			provide: async () => 'Use calendar data carefully.',
		},
		{ owner: 'calendar' },
	);
	const entry = registry.instructionEntries()[0];
	expect(entry).toMatchObject({
		category: 'instructions',
		owner: 'calendar',
		revision: 1,
		risk: 'medium',
	});
	expect(registry.isCurrent(entry!)).toBe(true);
	expect(dispose()).toBe(true);
	expect(registry.isCurrent(entry!)).toBe(false);
});
