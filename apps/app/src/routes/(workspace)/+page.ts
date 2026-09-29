import { redirect } from '@sveltejs/kit';
import { getAppPlatform } from '$lib/platform';

// The desktop app starts on Home. Redirecting in the load function happens
// before anything renders, so there is no blank page and no second
// navigation. The browser build keeps its own start screen at `/`.
export function load() {
	const platform = getAppPlatform();
	if (platform && platform !== 'web') redirect(307, '/inbox');
}
