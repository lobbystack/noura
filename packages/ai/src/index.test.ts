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
