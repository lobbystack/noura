import { redirect } from '@sveltejs/kit';
import { requestSettings } from '$lib/settings.svelte';

// Settings is a dialog. A link to /settings opens it over Home without
// rendering an interim page.
export function load() {
	requestSettings();
	redirect(307, '/inbox');
}
