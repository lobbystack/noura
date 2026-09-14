import type { PluginPlatform } from '@noura/plugin-sdk';
import {
	PluginRuntime,
	browserPluginCapabilities,
	type NouraClient,
} from '@noura/workspace';

/** Native composition entry point; activation/reconciliation is shared with web. */
export class AppPluginRuntime extends PluginRuntime {
	constructor(client: NouraClient, platform: PluginPlatform) {
		super(client, {
			platform,
			...(platform === 'web'
				? { supportedCapabilities: browserPluginCapabilities }
				: {}),
		});
	}
}
