export type Platform = 'macOS' | 'Windows' | 'Linux';

export interface SiteConfig {
	marketingUrl: string;
	githubUrl: string;
	releasesUrl: string;
	/** Legal details shown in the policies. */
	legal: {
		entity: string;
		address: string;
		contactEmail: string;
		privacyOfficer: string;
		governingLaw: string;
		websiteHost: string;
		/** ISO date (YYYY-MM-DD) the current policies took effect. Update it when they change. */
		effectiveDate: string;
	};
	/**
	 * Optional cookie categories that need consent. Keep this empty while the
	 * site sets no analytics or marketing cookies; the banner then only informs.
	 */
	optionalCookies: ReadonlyArray<{
		id: string;
		label: string;
		purpose: string;
	}>;
	/**
	 * Installers per platform. The release workflow uploads each file under a
	 * stable name, so these links always serve the latest published release.
	 */
	downloads: ReadonlyArray<{
		platform: Platform;
		detail: string;
		url: string;
		alternate?: { label: string; url: string };
		/** One short line about a first-launch warning the platform shows. */
		installNote?: string;
	}>;
}

const githubUrl = 'https://github.com/lobbystack/noura';
const latest = `${githubUrl}/releases/latest/download`;

export const siteConfig: SiteConfig = {
	marketingUrl: 'https://noura.app/',
	githubUrl,
	releasesUrl: `${githubUrl}/releases`,
	legal: {
		entity: 'Lobbystack Inc.',
		address: 'Saint-Nicolas, Québec, Canada',
		contactEmail: 'hello@noura.app',
		privacyOfficer: 'Raphaël Morency, President',
		governingLaw: 'the Province of Québec, Canada',
		websiteHost: 'Railway',
		effectiveDate: '2026-09-28',
	},
	optionalCookies: [],
	downloads: [
		{
			platform: 'macOS',
			detail: 'Apple silicon and Intel',
			url: `${latest}/Noura-macOS.dmg`,
			installNote:
				'The app isn’t notarized yet. If macOS blocks the first launch, open System Settings > Privacy & Security and choose Open Anyway.',
		},
		{
			platform: 'Windows',
			detail: 'Windows 10 and 11',
			url: `${latest}/Noura-Windows-Setup.exe`,
			installNote:
				'If Windows SmartScreen warns you, choose More info, then Run anyway.',
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
