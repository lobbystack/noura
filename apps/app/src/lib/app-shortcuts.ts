import type { AppMenuCommand } from '@noura/workspace';
import type { HostOs } from './host-os';

export interface ShortcutEvent {
	key: string;
	code?: string;
	metaKey: boolean;
	ctrlKey: boolean;
	shiftKey: boolean;
	altKey: boolean;
}

const GO_TO: Record<string, AppMenuCommand> = {
	'1': 'go-home',
	'2': 'go-files',
	'3': 'go-tasks',
	'4': 'go-calendar',
	'5': 'go-projects',
	'6': 'go-ai',
};

/**
 * The app command a key press runs. These match the macOS menu bar
 * (src-tauri/src/menu.rs); on Windows, Linux and the web the app handles them
 * itself. Mod is Command on a Mac and Control elsewhere.
 */
export function shortcutCommand(
	event: ShortcutEvent,
	os: HostOs,
): AppMenuCommand | null {
	if (event.altKey) return null;
	// Control+Tab switches tabs everywhere, as in browsers.
	if (event.ctrlKey && !event.metaKey && event.key === 'Tab')
		return event.shiftKey ? 'previous-tab' : 'next-tab';
	const mod = os === 'mac' ? event.metaKey : event.ctrlKey;
	const other = os === 'mac' ? event.ctrlKey : event.metaKey;
	if (!mod || other) return null;
	const key = event.key.toLowerCase();
	if (event.shiftKey) {
		if (event.code === 'BracketRight' || key === '}' || key === ']')
			return 'next-tab';
		if (event.code === 'BracketLeft' || key === '{' || key === '[')
			return 'previous-tab';
		if (key === 'n') return 'new-folder';
		if (key === 'f') return 'search';
		return null;
	}
	if (key === 'n') return 'new-note';
	if (key === 'o') return 'quick-open';
	if (key === 'w') return 'close-tab';
	if (key === '\\' || event.code === 'Backslash') return 'toggle-sidebar';
	return GO_TO[key] ?? null;
}

/** Where a command came from: a key press or the macOS menu bar. */
export type ShortcutSource = 'key' | 'menu';

/**
 * On a Mac the menu bar and the key press can both report one shortcut.
 * The returned check lets the second report of a pair through only when it
 * comes from the same source, so pressing a shortcut twice quickly (Cmd-W
 * to close two tabs) still runs it twice.
 */
export function shortcutOnce(windowMs = 300) {
	const last = new Map<string, { at: number; source: ShortcutSource }>();
	return (command: string, source: ShortcutSource, now: number): boolean => {
		const previous = last.get(command);
		if (previous && previous.source !== source && now - previous.at < windowMs)
			return false;
		last.set(command, { at: now, source });
		return true;
	};
}

/** Shortcuts as listed in Settings, in the order they are shown. */
export const APP_SHORTCUTS: ReadonlyArray<{
	label: string;
	keys: string[];
}> = [
	{ label: 'New note', keys: ['Mod', 'N'] },
	{ label: 'New folder', keys: ['Mod', 'Shift', 'N'] },
	{ label: 'Open a file by name', keys: ['Mod', 'O'] },
	{ label: 'Search file contents', keys: ['Mod', 'Shift', 'F'] },
	{ label: 'Search and run commands', keys: ['Mod', 'K'] },
	{ label: 'Rename the open file', keys: ['F2'] },
	{ label: 'Close tab', keys: ['Mod', 'W'] },
	{ label: 'Next tab', keys: ['Ctrl', 'Tab'] },
	{ label: 'Previous tab', keys: ['Ctrl', 'Shift', 'Tab'] },
	{ label: 'Show or hide the sidebar', keys: ['Mod', '\\'] },
	{
		label: 'Go to Home, Files, Tasks, Calendar, Projects, AI',
		keys: ['Mod', '1–6'],
	},
	{ label: 'Open settings', keys: ['Mod', ','] },
];
