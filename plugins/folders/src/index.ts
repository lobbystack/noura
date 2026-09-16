import { definePlugin } from '@noura/plugin-sdk';

const toolDisposers = new WeakMap<object, Array<() => boolean>>();

export default definePlugin({
	manifest: {
		id: 'folders',
		name: 'Folders',
		version: '0.1.0',
		capabilities: ['workspace.files', 'workspace.events', 'ai.tools'],
		platforms: ['desktop'],
	},
	activate(context) {
		const disposers = [
			context.ai.registerTool({
				name: 'folders.list',
				description: 'List workspace files and folders.',
				inputSchema: {
					type: 'object',
					additionalProperties: false,
				},
				risk: 'low',
				execute: () => context.files.list(),
			}),
		];
		toolDisposers.set(context, disposers);
	},
	deactivate(context) {
		for (const dispose of toolDisposers.get(context)?.splice(0) ?? []) {
			dispose();
		}
	},
});
