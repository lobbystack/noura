import { createContext } from 'svelte';

/** Transient settings UI, scoped to the app layout. */
export class SettingsDialog {
	open = $state(false);
	returnFocus: HTMLElement | null = null;

	show() {
		if (this.open) return;
		this.returnFocus =
			document.activeElement instanceof HTMLElement
				? document.activeElement
				: null;
		this.open = true;
	}
}

export const [getSettingsDialog, setSettingsDialog] =
	createContext<SettingsDialog>();
