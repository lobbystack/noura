/** Parse a chosen file as JSON, or return `undefined` when it isn't JSON. */
export async function readJsonFile(file: File): Promise<unknown> {
	try {
		return JSON.parse(await file.text()) as unknown;
	} catch {
		return undefined;
	}
}

/** Save a value as a JSON download and release the object URL afterward. */
export function downloadJson(value: unknown, filename: string): void {
	const blob = new Blob([JSON.stringify(value)], { type: 'application/json' });
	const url = URL.createObjectURL(blob);
	const anchor = document.createElement('a');
	anchor.href = url;
	anchor.download = filename;
	anchor.hidden = true;
	document.body.append(anchor);
	try {
		anchor.click();
	} finally {
		anchor.remove();
		// Give the browser time to start the download before revoking.
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
	}
}

/** A download name for an exported recovery kit. */
export function recoveryKitFilename(label: string | null): string {
	const safe = (label ?? 'browser')
		.replace(/[^a-zA-Z0-9_-]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 64);
	return `noura-recovery-kit-${safe || 'browser'}.noura-recovery-kit.json`;
}
