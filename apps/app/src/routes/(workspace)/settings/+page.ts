import { redirect } from '@sveltejs/kit';
import { getAppPlatform } from '$lib/platform';
import { requestSettings } from '$lib/settings.svelte';

// Settings is a dialog in the desktop app. A link to /settings opens it
// over Home without rendering an interim page. The browser build renders
// its own settings screen at this path.
export function load() {
	const platform = getAppPlatform();
	if (!platform || platform === 'web') return;
	requestSettings();
	redirect(307, '/inbox');
}
