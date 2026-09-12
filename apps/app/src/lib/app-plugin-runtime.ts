import { supportsPlatform, type PluginPlatform } from '@noura/plugin-sdk';
import {
	PluginHost,
	PluginRuntime,
	createPluginHostServices,
	firstPartyPlugins,
	type NouraClient,
	type PluginSyncResult,
} from '@noura/workspace';

/** App composition adapter until the shared runtime accepts a platform/catalog filter. */
export class AppPluginRuntime extends PluginRuntime {
	constructor(
		private readonly client: NouraClient,
		platform: PluginPlatform,
	) {
		super(
			client,
			new PluginHost(createPluginHostServices(client), { platform }),
		);
	}

	override async syncWithManifest(): Promise<PluginSyncResult> {
		const manifest = await this.client.manifest.read();
		const enabled = new Set(manifest.enabledPlugins);
		const deactivated: string[] = [];
		for (const active of this.host.activeManifests()) {
			if (!enabled.has(active.id)) {
				if (await this.host.deactivate(active.id)) deactivated.push(active.id);
			}
		}
		const activated: string[] = [];
		for (const plugin of firstPartyPlugins) {
			if (
				supportsPlatform(plugin.manifest, this.host.platform) &&
				enabled.has(plugin.manifest.id) &&
				!this.host.isActive(plugin.manifest.id)
			) {
				await this.host.activate(plugin);
				activated.push(plugin.manifest.id);
			}
		}
		// Preserve unsupported and unknown IDs on disk; availability is device-local.
		return {
			activated,
			deactivated,
			enabledPluginIds: manifest.enabledPlugins,
		};
	}
}
