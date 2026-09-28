import { relaunch } from '@tauri-apps/plugin-process';
import { check } from '@tauri-apps/plugin-updater';
import type { AppUpdaterAdapter } from './app-updater';

export function createTauriAppUpdater(): AppUpdaterAdapter {
	return {
		async check() {
			const update = await check();
			if (!update) return null;
			return {
				version: update.version,
				notes: update.body,
				download: () => update.download(),
				install: () => update.install(),
			};
		},
		restart: () => relaunch(),
	};
}
