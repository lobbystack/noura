import { expect, test } from 'bun:test';
import { normalizePdfPage, pdfHref } from './navigation';

test('encodes file locators and clamps invalid page links', () => {
	const url = new URL(
		pdfHref('cours/évaluation #1.pdf', 7),
		'https://noura.test',
	);
	expect(url.pathname).toBe('/pdf');
	expect(url.searchParams.get('path')).toBe('cours/évaluation #1.pdf');
	expect(url.searchParams.get('page')).toBe('7');
	for (const input of [undefined, null, 'bad', 0, -1, 1.5, Infinity])
		expect(normalizePdfPage(input, 20)).toBe(1);
	expect(normalizePdfPage(200, 20)).toBe(20);
});
