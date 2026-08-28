import { definePlugin } from '@noura/plugin-sdk';
export default definePlugin({
	manifest: {
		id: 'notes',
		name: 'Notes',
		version: '0.1.0',
		capabilities: [
			'workspace.objects',
			'workspace.search',
			'workspace.commands',
			'workspace.events',
		],
	},
	activate() {},
});
