import type { AppCapabilities } from '@noura/workspace';
import { getAppPlatform } from './platform';
import { getNouraClient } from './state.svelte';

/** What to assume until the host answers, so menus don't flicker. */
function initial(): AppCapabilities {
	const native = getAppPlatform() !== 'web';
	return {
		openTerminal: false,
		revealSelectsFile: native,
		revealInFileManager: native,
		openWithDefaultApp: native,
		systemTrash: native,
		workspaceFolders: native,
		mcp: native,
	};
}

/**
 * What the host can do, so the interface hides actions it can't perform
 * (revealing files, opening other apps, picking folders) instead of showing
 * a separate interface per platform.
 */
class HostCapabilities {
	#value = $state.raw<AppCapabilities>(initial());
	#loading: Promise<void> | null = null;

	get current(): AppCapabilities {
		void this.load();
		return this.#value;
	}

	load(): Promise<void> {
		this.#loading ??= Promise.resolve()
			.then(() => getNouraClient().app.capabilities())
			.then((value) => {
				this.#value = value;
			})
			.catch(() => {
				// Keep the platform defaults.
			});
		return this.#loading;
	}
}

export const hostCapabilities = new HostCapabilities();
