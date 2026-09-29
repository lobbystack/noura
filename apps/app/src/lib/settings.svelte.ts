import { createContext } from 'svelte';
import type { SettingsSectionId } from '$lib/settings-sections';
export type SettingsSection = SettingsSectionId;

/** Transient settings UI, scoped to the app layout. */
export class SettingsDialog {
	open = $state(false);
	requestedSection = $state<SettingsSection | null>(null);
	requestId = $state(0);
	returnFocus: HTMLElement | null = null;

	constructor() {
		active = this;
		if (pending !== undefined) {
			const section = pending;
			pending = undefined;
			this.show(section ?? undefined);
		}
	}

	showSync() {
		this.show('sync');
	}

	showAccount() {
		this.show('account');
	}

	/** Open settings, on a given section when one is named. */
	show(section?: SettingsSection) {
		if (section) {
			this.requestedSection = section;
			this.requestId += 1;
		}
		if (this.open) return;
		if (!section) this.requestedSection = null;
		this.returnFocus =
			document.activeElement instanceof HTMLElement
				? document.activeElement
				: null;
		this.open = true;
	}
}

let active: SettingsDialog | null = null;
/** A request made before the app layout created its dialog. */
let pending: SettingsSection | null | undefined;

/**
 * Open settings from outside a component, such as a route load. The request
 * waits for the app layout when its dialog does not exist yet.
 */
export function requestSettings(section?: SettingsSection) {
	if (active) active.show(section);
	else pending = section ?? null;
}

export const [getSettingsDialog, setSettingsDialog] =
	createContext<SettingsDialog>();
