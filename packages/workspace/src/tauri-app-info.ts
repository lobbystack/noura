import { getVersion } from '@tauri-apps/api/app';
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart';

/** Facts about the installed desktop app and its operating-system integration. */
export interface DesktopAppAdapter {
	version(): Promise<string>;
	launchAtLogin: {
		isEnabled(): Promise<boolean>;
		setEnabled(enabled: boolean): Promise<void>;
	};
}

export function createTauriDesktopApp(): DesktopAppAdapter {
	return {
		version: () => getVersion(),
		launchAtLogin: {
			isEnabled: () => isEnabled(),
			setEnabled: (enabled) => (enabled ? enable() : disable()),
		},
	};
}
