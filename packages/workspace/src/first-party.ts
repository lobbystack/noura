import calendar from '@noura/plugin-calendar';
import folders from '@noura/plugin-folders';
import notes from '@noura/plugin-notes';
import projects from '@noura/plugin-projects';
import tasks from '@noura/plugin-tasks';
import type { PluginHost } from '@noura/plugin-sdk';

export const firstPartyPlugins = [
	folders,
	notes,
	tasks,
	calendar,
	projects,
] as const;

export async function activateFirstPartyPlugins(
	host: PluginHost,
): Promise<void> {
	for (const plugin of firstPartyPlugins) await host.activate(plugin);
}
