/** Routing hint only. Native resolution is authoritative for paths. */
export function isPdfTarget(target: string): boolean {
	if (/^[a-z][a-z\d+.-]*:/i.test(target) || target.startsWith('//'))
		return false;
	try {
		return /\.pdf$/i.test(decodeURIComponent(target.split('#')[0] ?? ''));
	} catch {
		return false;
	}
}
