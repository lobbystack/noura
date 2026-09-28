export type Platform = 'macOS' | 'Windows' | 'Linux';

export interface SiteConfig {
	marketingUrl: string;
	githubUrl: string;
	releasesUrl: string;
	/**
	 * Installers per platform. The release workflow uploads each file under a
	 * stable name, so these links always serve the latest published release.
	 */
	downloads: ReadonlyArray<{
		platform: Platform;
		detail: string;
		url: string;
		alternate?: { label: string; url: string };
	}>;
}

const githubUrl = 'https://github.com/lobbystack/noura';
const latest = `${githubUrl}/releases/latest/download`;

export const siteConfig: SiteConfig = {
	marketingUrl: 'https://noura.app/',
	githubUrl,
	releasesUrl: `${githubUrl}/releases`,
	downloads: [
		{
			platform: 'macOS',
			detail: 'Apple silicon and Intel',
			url: `${latest}/Noura-macOS.dmg`,
		},
		{
			platform: 'Windows',
			detail: 'Windows 10 and 11',
			url: `${latest}/Noura-Windows-Setup.exe`,
		},
		{
			platform: 'Linux',
			detail: 'AppImage for x86_64',
			url: `${latest}/Noura-Linux.AppImage`,
			alternate: { label: '.deb', url: `${latest}/Noura-Linux.deb` },
		},
	],
};

/** Best-effort guess for the default download button. Never used for access decisions. */
export function detectPlatform(): Platform | null {
	if (typeof navigator === 'undefined') return null;
	const agent = navigator as Navigator & {
		userAgentData?: { platform?: string };
	};
	const hint = `${agent.userAgentData?.platform ?? ''} ${navigator.userAgent}`;
	if (/iPhone|iPad|Android/i.test(hint)) return null;
	if (/Mac/i.test(hint)) return 'macOS';
	if (/Win/i.test(hint)) return 'Windows';
	if (/Linux|X11/i.test(hint)) return 'Linux';
	return null;
}
