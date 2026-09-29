import { createContext } from 'svelte';
import {
	createAppUpdater,
	createTauriAppUpdater,
	createTauriDesktopApp,
	type AppUpdateState,
	type AppUpdater,
} from '@noura/workspace';
import { getAppPlatform } from '$lib/platform';
import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';

/**
 * Reactive view of the desktop updater, shared by the update notice and the
 * Updates settings. The update rules live in `createAppUpdater`.
 */
export class AppUpdates {
	state = $state.raw<AppUpdateState>({ status: 'idle' });
	version = $state<string | null>(null);
	/** Development builds and non-desktop hosts have no signed release to compare. */
	readonly supported: boolean;
	#updater: AppUpdater | null = null;

	constructor() {
		const desktop = getAppPlatform() === 'desktop';
		this.supported = desktop && !import.meta.env.DEV;
		if (desktop) {
			void createTauriDesktopApp()
				.version()
				.then((version) => (this.version = version))
				.catch(() => {});
		}
		if (this.supported) {
			this.#updater = createAppUpdater(createTauriAppUpdater(), {
				flush: flushPendingDrafts,
				onChange: (state) => (this.state = state),
			});
		}
	}

	check() {
		return this.#updater?.check();
	}

	installAndRestart() {
		return this.#updater?.installAndRestart();
	}
}

export const [getAppUpdates, setAppUpdates] = createContext<AppUpdates>();
