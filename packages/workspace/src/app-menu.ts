import { listen } from '@tauri-apps/api/event';

/**
 * Commands the macOS menu bar sends (see src-tauri/src/menu.rs). The app runs
 * the same commands from keyboard shortcuts on every platform.
 */
export const APP_MENU_COMMANDS = [
	'new-note',
	'new-folder',
	'quick-open',
	'search',
	'close-tab',
	'settings',
	'toggle-sidebar',
	'go-home',
	'go-files',
	'go-tasks',
	'go-calendar',
	'go-projects',
	'go-ai',
	'next-tab',
	'previous-tab',
] as const;

export type AppMenuCommand = (typeof APP_MENU_COMMANDS)[number];

export function isAppMenuCommand(value: unknown): value is AppMenuCommand {
	return (
		typeof value === 'string' &&
		(APP_MENU_COMMANDS as readonly string[]).includes(value)
	);
}

/** Listen for menu bar commands. Resolves to the unsubscribe function. */
export async function subscribeAppMenu(
	handler: (command: AppMenuCommand) => void,
): Promise<() => void> {
	return listen<unknown>('app-menu', (event) => {
		if (isAppMenuCommand(event.payload)) handler(event.payload);
	});
}
