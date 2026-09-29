/**
 * Settings sections and the rules that decide which ones appear. Like the
 * sidebar registry, sections that belong to a plugin appear only while it is
 * enabled. Labels and icons stay with the settings component.
 */
export type SettingsSectionId =
	| 'general'
	| 'editor'
	| 'shortcuts'
	| 'account'
	| 'workspace'
	| 'plugins'
	| 'ai'
	| 'external-tools'
	| 'sync'
	| 'people'
	| 'files'
	| 'about';

export interface SettingsSection {
	id: SettingsSectionId;
	group: 'device' | 'workspace' | 'app';
	/** Plugin whose enabled state gates the section; null = always shown. */
	pluginId: string | null;
	/** Also shown while a device account is signed in, so you can sign out. */
	shownWhenSignedIn?: boolean;
	desktopOnly?: boolean;
}

const SETTINGS_SECTIONS: readonly SettingsSection[] = [
	{ id: 'general', group: 'device', pluginId: null },
	{ id: 'editor', group: 'device', pluginId: null },
	{ id: 'shortcuts', group: 'device', pluginId: null },
	{ id: 'account', group: 'device', pluginId: 'sync', shownWhenSignedIn: true },
	{ id: 'workspace', group: 'workspace', pluginId: null },
	{ id: 'plugins', group: 'workspace', pluginId: null },
	{ id: 'ai', group: 'workspace', pluginId: null },
	{
		id: 'external-tools',
		group: 'workspace',
		pluginId: null,
		desktopOnly: true,
	},
	{ id: 'sync', group: 'workspace', pluginId: 'sync' },
	{ id: 'people', group: 'workspace', pluginId: 'sync' },
	{ id: 'files', group: 'workspace', pluginId: null },
	{ id: 'about', group: 'app', pluginId: null },
];

export interface SettingsVisibility {
	desktop: boolean;
	enabledPluginIds: ReadonlySet<string>;
	/** Whether this device is signed in to an account. */
	signedIn: boolean;
}

function settingsSectionVisible(
	section: SettingsSection,
	visibility: SettingsVisibility,
): boolean {
	if (section.desktopOnly && !visibility.desktop) return false;
	if (!section.pluginId || visibility.enabledPluginIds.has(section.pluginId))
		return true;
	return !!section.shownWhenSignedIn && visibility.signedIn;
}

/** The sections to show, in registry order. */
export function visibleSettingsSections(
	visibility: SettingsVisibility,
): SettingsSection[] {
	return SETTINGS_SECTIONS.filter((section) =>
		settingsSectionVisible(section, visibility),
	);
}
