import { siteConfig } from '$lib/site-config';

const STORAGE_KEY = 'noura-cookie-consent';
/** Bump when the optional cookie categories change, so visitors decide again. */
const CONSENT_VERSION = 1;

export interface CookieConsent {
	version: number;
	decidedAt: string;
	/** Optional category id to granted. Essential storage needs no consent. */
	granted: Record<string, boolean>;
}

export const consent = $state<{
	value: CookieConsent | null;
	loaded: boolean;
	settingsOpen: boolean;
}>({ value: null, loaded: false, settingsOpen: false });

export function loadConsent() {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		const parsed = raw ? (JSON.parse(raw) as CookieConsent) : null;
		consent.value = parsed?.version === CONSENT_VERSION ? parsed : null;
	} catch {
		consent.value = null;
	}
	consent.loaded = true;
}

export function saveConsent(granted: Record<string, boolean>) {
	const value: CookieConsent = {
		version: CONSENT_VERSION,
		decidedAt: new Date().toISOString(),
		granted,
	};
	consent.value = value;
	consent.settingsOpen = false;
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
	} catch {
		// Private browsing can block storage. The choice still applies to this visit.
	}
}

export function acceptAll() {
	saveConsent(
		Object.fromEntries(siteConfig.optionalCookies.map((c) => [c.id, true])),
	);
}

export function rejectOptional() {
	saveConsent(
		Object.fromEntries(siteConfig.optionalCookies.map((c) => [c.id, false])),
	);
}

/** Check before loading any script that sets an optional cookie. */
export function hasConsent(categoryId: string): boolean {
	return consent.value?.granted[categoryId] === true;
}

export function openCookieSettings() {
	consent.settingsOpen = true;
}
