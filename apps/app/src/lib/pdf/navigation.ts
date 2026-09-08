export function pdfHref(relativePath: string, page?: number): string {
	const query = new URLSearchParams({ path: relativePath });
	if (page !== undefined) query.set('page', String(normalizePdfPage(page)));
	return `/pdf?${query}`;
}
export function normalizePdfPage(
	page: unknown,
	count = Number.MAX_SAFE_INTEGER,
): number {
	const value = Number(page);
	return Number.isSafeInteger(value) && value > 0 ? Math.min(value, count) : 1;
}
export interface PdfPosition {
	page: number;
	scale: string;
}
