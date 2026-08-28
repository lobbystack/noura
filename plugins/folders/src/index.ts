import { definePlugin } from '@noura/plugin-sdk';
export default definePlugin({
	manifest: {
		id: 'folders',
		name: 'Folders',
		version: '0.1.0',
		capabilities: ['workspace.files', 'workspace.events'],
	},
	activate() {},
});
