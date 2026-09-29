import { createContext } from 'svelte';

/** Transient settings UI, scoped to the app layout. */
export class SettingsDialog {
	open = $state(false);
	requestedSection = $state<'sync' | 'account' | null>(null);
	requestId = $state(0);

	showSync() {
		this.showSection('sync');
	}

	showAccount() {
		this.showSection('account');
	}

	showSection(section: 'sync' | 'account') {
		this.requestedSection = section;
		this.requestId += 1;
		this.show();
	}
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
