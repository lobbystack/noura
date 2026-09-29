/** The plugin that turns browser sync on for a workspace. */
export const SYNC_PLUGIN_ID = 'sync';

/**
 * Records, per browser and workspace, that the sync plugin state was set.
 * It lives in the adapter directory, outside canonical workspace files.
 */
export interface BrowserSyncPluginMarker {
	exists(): Promise<boolean>;
	write(): Promise<void>;
}

export interface BrowserSyncPluginMigration {
	/** Whether this browser already has a sync binding for the workspace. */
	hasBinding(): Promise<boolean>;
	marker: BrowserSyncPluginMarker;
	registry: {
		read(): Promise<{ enabledPluginIds: string[]; updated: string }>;
		setEnabled(
			pluginId: string,
			enabled: boolean,
			expectedUpdated: string,
		): Promise<unknown>;
	};
}

/**
 * Browser workspaces that set up sync before the plugin existed keep syncing.
 * The first open turns the plugin on once and records a marker, so a later
 * "off" is never undone. Returns whether the plugin was turned on.
 */
export async function migrateBrowserSyncPlugin(
	migration: BrowserSyncPluginMigration,
): Promise<boolean> {
	if (await migration.marker.exists()) return false;
	if (!(await migration.hasBinding())) return false;
	const preference = await migration.registry.read();
	let enabled = false;
	// A crash after the manifest write leaves the plugin on without a
	// marker; then only the marker is written.
	if (!preference.enabledPluginIds.includes(SYNC_PLUGIN_ID)) {
		await migration.registry.setEnabled(
			SYNC_PLUGIN_ID,
			true,
			preference.updated,
		);
		enabled = true;
	}
	await migration.marker.write();
	return enabled;
}
