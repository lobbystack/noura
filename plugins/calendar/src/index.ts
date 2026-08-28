import { definePlugin } from '@noura/plugin-sdk';
export default definePlugin({
	manifest: {
		id: 'calendar',
		name: 'Calendar',
		version: '0.1.0',
		capabilities: ['workspace.objects', 'workspace.events'],
	},
	activate() {},
});
