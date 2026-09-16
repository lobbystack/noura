import { definePlugin } from '@noura/plugin-sdk';

/** Gates the AI workspace surface; provider setup remains device-local. */
export default definePlugin({
	manifest: {
		id: 'ai',
		name: 'AI',
		version: '0.1.0',
		capabilities: [],
		platforms: ['desktop'],
	},
	activate() {},
});
