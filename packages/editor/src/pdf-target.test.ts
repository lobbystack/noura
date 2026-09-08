import { expect, test } from 'bun:test';
import { isPdfTarget } from './pdf-target';

test('recognizes local PDF links without enabling remote embeds', () => {
	for (const value of [
		'./lecture.pdf#page=7',
		'file%20name.PDF',
		'lecture%2Epdf',
	])
		expect(isPdfTarget(value)).toBe(true);
	for (const value of [
		'https://example.com/a.pdf',
		'//example.com/a.pdf',
		'data:application/pdf,x',
		'note.md',
		'photo.png',
		'bad%xx.pdf',
	])
		expect(isPdfTarget(value)).toBe(false);
});
