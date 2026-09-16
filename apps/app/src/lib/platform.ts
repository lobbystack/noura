import { isTauri } from '@tauri-apps/api/core';
import type { PluginPlatform } from '@noura/plugin-sdk';

/**
 * The Tauri CLI supplies the compilation target to its frontend hooks. Pair
 * that with the public runtime check: even a native build opened in a browser
 * is web. Never infer support from viewport size or a spoofable user agent.
 * Missing native build metadata is unknown, not an implicit desktop grant.
 */
export function detectPlatform(
	native: boolean,
	target?: string,
): PluginPlatform | null {
	if (!native) return 'web';
	if (target === 'android' || target === 'ios') return 'mobile';
	if (target === 'darwin' || target === 'windows' || target === 'linux')
		return 'desktop';
	return null;
}

export function getAppPlatform(): PluginPlatform | null {
	return detectPlatform(isTauri(), import.meta.env.NOURA_TAURI_PLATFORM);
}

export const platformLabels: Record<PluginPlatform, string> = {
	desktop: 'Desktop',
	mobile: 'Mobile',
	web: 'Web',
};
