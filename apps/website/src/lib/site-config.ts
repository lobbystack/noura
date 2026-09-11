export interface SiteConfig {
	marketingUrl: string;
	demoUrl: string;
	githubUrl: string;
	downloads: ReadonlyArray<{
		platform: 'macOS' | 'Windows' | 'Linux';
		detail: string;
		url?: string;
	}>;
}

export const siteConfig: SiteConfig = {
	marketingUrl: 'https://noura.app',
	demoUrl: dev ? 'http://127.0.0.1:5173' : 'https://demo.noura.app',
	githubUrl: 'https://github.com/lobbystack/noura',
	downloads: [
		{ platform: 'macOS', detail: 'Apple silicon and Intel' },
		{ platform: 'Windows', detail: 'Windows 10 and later' },
		{ platform: 'Linux', detail: 'AppImage and deb' },
	],
};
import { dev } from '$app/environment';
