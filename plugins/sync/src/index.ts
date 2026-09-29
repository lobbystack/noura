import { definePlugin } from '@noura/plugin-sdk';

/**
 * Turns sync and live collaboration on for this workspace replica. The
 * native host reads the same `enabled_plugins` entry, so with the plugin off
 * no sync loop, credential read, or collaboration session runs. Browsers
 * activate it without collaboration: browser sync has its own settings.
 */
export default definePlugin({
	manifest: {
		id: 'sync',
		name: 'Sync',
		version: '0.1.0',
		capabilities: ['workspace.collaboration'],
		platforms: ['desktop', 'mobile', 'web'],
		activationCapabilities: { web: [] },
	},
	activate(context) {
		if (context.hasCapability('workspace.collaboration'))
			context.collaboration.registerProvider(context.collaboration.service);
	},
});
