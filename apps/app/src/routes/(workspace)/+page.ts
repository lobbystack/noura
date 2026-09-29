import { redirect } from '@sveltejs/kit';

// The app starts on Home. Redirecting in the load function happens before
// anything renders, so there is no blank page and no second navigation.
export function load() {
	redirect(307, '/inbox');
}
