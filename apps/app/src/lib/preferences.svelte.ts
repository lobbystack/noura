import { browser } from '$app/environment';

/**
 * Preferences for this device only. They never enter workspace files, so each
 * computer keeps its own editor comfort settings.
 */
const STORAGE_KEY = 'noura.device-preferences';

export type TextSize = 'small' | 'default' | 'large';
export type LineWidth = 'narrow' | 'default' | 'full';

const textSizes: Record<TextSize, string> = {
	small: '0.9375rem',
	default: '1rem',
	large: '1.125rem',
};
const lineWidths: Record<LineWidth, string> = {
	narrow: '40rem',
	default: '48rem',
	full: 'none',
};

interface Stored {
	textSize?: TextSize;
	lineWidth?: LineWidth;
	spellcheck?: boolean;
}

class DevicePreferences {
	textSize = $state<TextSize>('default');
	lineWidth = $state<LineWidth>('default');
	spellcheck = $state(false);

	readonly textSizeValue = $derived(textSizes[this.textSize]);
	readonly lineWidthValue = $derived(lineWidths[this.lineWidth]);

	constructor() {
		if (!browser) return;
		try {
			const stored = JSON.parse(
				localStorage.getItem(STORAGE_KEY) ?? '{}',
			) as Stored;
			if (stored.textSize && stored.textSize in textSizes)
				this.textSize = stored.textSize;
			if (stored.lineWidth && stored.lineWidth in lineWidths)
				this.lineWidth = stored.lineWidth;
			if (typeof stored.spellcheck === 'boolean')
				this.spellcheck = stored.spellcheck;
		} catch {
			// Unreadable storage falls back to the defaults.
		}
	}

	update(patch: Stored) {
		if (patch.textSize) this.textSize = patch.textSize;
		if (patch.lineWidth) this.lineWidth = patch.lineWidth;
		if (typeof patch.spellcheck === 'boolean')
			this.spellcheck = patch.spellcheck;
		try {
			localStorage.setItem(
				STORAGE_KEY,
				JSON.stringify({
					textSize: this.textSize,
					lineWidth: this.lineWidth,
					spellcheck: this.spellcheck,
				} satisfies Stored),
			);
		} catch {
			// The choice still applies until the app closes.
		}
	}
}

export const preferences = new DevicePreferences();
