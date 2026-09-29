export type HostOs = 'mac' | 'windows' | 'linux';

/**
 * The operating system for labels and shortcut hints only. Desktop builds know
 * their target; the browser build falls back to the user agent.
 */
export function hostOs(
	target: string = import.meta.env.NOURA_TAURI_PLATFORM ?? '',
	userAgent: string = typeof navigator === 'undefined'
		? ''
		: navigator.userAgent,
): HostOs {
	if (target === 'darwin' || target === 'ios') return 'mac';
	if (target === 'windows') return 'windows';
	if (target === 'linux' || target === 'android') return 'linux';
	if (/Mac|iPhone|iPad/.test(userAgent)) return 'mac';
	if (/Windows/.test(userAgent)) return 'windows';
	return 'linux';
}

/** What the file manager is called here, for "Reveal in Finder" and similar. */
export function revealLabel(os: HostOs = hostOs()): string {
	if (os === 'mac') return 'Reveal in Finder';
	if (os === 'windows') return 'Show in Explorer';
	return 'Show in file manager';
}

const MAC_KEYS: Record<string, string> = {
	Mod: '⌘',
	Shift: '⇧',
	Alt: '⌥',
	Ctrl: '⌃',
	Enter: '↩',
	Backspace: '⌫',
};

const OTHER_KEYS: Record<string, string> = { Mod: 'Ctrl' };

/**
 * Keys for display, one entry per key: `['Mod', 'K']` shows as `⌘ K` on a Mac
 * and `Ctrl K` elsewhere.
 */
export function shortcutKeys(
	keys: readonly string[],
	os: HostOs = hostOs(),
): string[] {
	const names = os === 'mac' ? MAC_KEYS : OTHER_KEYS;
	return keys.map((key) => names[key] ?? key);
}

/** One string for tooltips: `⌘K` on a Mac, `Ctrl+K` elsewhere. */
export function shortcutLabel(
	keys: readonly string[],
	os: HostOs = hostOs(),
): string {
	return shortcutKeys(keys, os).join(os === 'mac' ? '' : '+');
}
