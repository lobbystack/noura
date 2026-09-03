import ai from '@noura/plugin-ai';
import calendar from '@noura/plugin-calendar';
import folders from '@noura/plugin-folders';
import notes from '@noura/plugin-notes';
import projects from '@noura/plugin-projects';
import tasks from '@noura/plugin-tasks';

/**
 * The whole activation path belongs to `PluginRuntime.syncWithManifest`:
 * every plugin, including these bundled domains, is driven by
 * `enabled_plugins` in workspace.yaml.
 */
export const firstPartyPlugins = [
	ai,
	folders,
	notes,
	tasks,
	calendar,
	projects,
] as const;
