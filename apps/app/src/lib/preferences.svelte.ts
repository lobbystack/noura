import { browser } from '$app/environment';
import { ZOOM_LEVELS } from './zoom';

export { nextZoom, ZOOM_LEVELS } from './zoom';

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

/** The sidebar's narrowest width in CSS pixels, and its width until resized. */
export const SIDEBAR_MIN_WIDTH = 224;

interface Stored {
	textSize?: TextSize;
	lineWidth?: LineWidth;
	spellcheck?: boolean;
	sidebarWidth?: number;
	zoom?: number;
	/** False when the user hid the sidebar with Mod+\. */
	sidebarOpen?: boolean;
	/** Sidebar sections the user folded, by section key. */
	collapsedSections?: string[];
}

class DevicePreferences {
	textSize = $state<TextSize>('default');
	lineWidth = $state<LineWidth>('default');
	spellcheck = $state(false);
	sidebarWidth = $state(SIDEBAR_MIN_WIDTH);
	zoom = $state(1);
	sidebarOpen = $state(true);
	collapsedSections = $state.raw<readonly string[]>([]);

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
			if (
				typeof stored.sidebarWidth === 'number' &&
				stored.sidebarWidth >= SIDEBAR_MIN_WIDTH
			)
				this.sidebarWidth = Math.round(stored.sidebarWidth);
			if (ZOOM_LEVELS.some((level) => level === stored.zoom))
				this.zoom = stored.zoom!;
			if (typeof stored.sidebarOpen === 'boolean')
				this.sidebarOpen = stored.sidebarOpen;
			if (Array.isArray(stored.collapsedSections))
				this.collapsedSections = stored.collapsedSections.filter(
					(key) => typeof key === 'string',
				);
		} catch {
			// Unreadable storage falls back to the defaults.
		}
	}

	update(patch: Stored) {
		if (patch.textSize) this.textSize = patch.textSize;
		if (patch.lineWidth) this.lineWidth = patch.lineWidth;
		if (typeof patch.spellcheck === 'boolean')
			this.spellcheck = patch.spellcheck;
		if (typeof patch.sidebarWidth === 'number')
			this.sidebarWidth = Math.max(
				SIDEBAR_MIN_WIDTH,
				Math.round(patch.sidebarWidth),
			);
		if (ZOOM_LEVELS.some((level) => level === patch.zoom))
			this.zoom = patch.zoom!;
		if (typeof patch.sidebarOpen === 'boolean')
			this.sidebarOpen = patch.sidebarOpen;
		if (patch.collapsedSections)
			this.collapsedSections = [...new Set(patch.collapsedSections)];
		try {
			localStorage.setItem(
				STORAGE_KEY,
				JSON.stringify({
					textSize: this.textSize,
					lineWidth: this.lineWidth,
					spellcheck: this.spellcheck,
					sidebarWidth: this.sidebarWidth,
					zoom: this.zoom,
					sidebarOpen: this.sidebarOpen,
					collapsedSections: [...this.collapsedSections],
				} satisfies Stored),
			);
		} catch {
			// The choice still applies until the app closes.
		}
	}
}

export const preferences = new DevicePreferences();

export function isSectionCollapsed(key: string): boolean {
	return preferences.collapsedSections.includes(key);
}

export function toggleSection(key: string) {
	preferences.update({
		collapsedSections: isSectionCollapsed(key)
			? preferences.collapsedSections.filter((value) => value !== key)
			: [...preferences.collapsedSections, key],
	});
}
