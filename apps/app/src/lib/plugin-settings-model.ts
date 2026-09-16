import type { PluginManifest, PluginPlatform } from '@noura/plugin-sdk';

/** Presentation contract shared by native and browser adapters. */
export interface PluginSettingsModel {
	readonly platform: PluginPlatform | null;
	readonly catalog: readonly PluginManifest[];
	readonly orderedPluginIds: readonly string[];
	readonly enabledIds: readonly string[];
	readonly synced: boolean;
	readonly lastError: string | null;
	isSupported(id: string): boolean;
	isEnabled(id: string): boolean;
	unavailableReason(id: string): string | null;
	sync(): Promise<void>;
	setEnabled(id: string, enabled: boolean): Promise<void>;
}
