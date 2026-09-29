import { expect, test } from 'bun:test';
import { quickOpen, quickOpenScore } from './quick-open';

const paths = [
	'Projects/Launch brief.md',
	'Archive/launch-notes-2023.md',
	'Daily/2026-09-28.md',
	'Reading/Books/The Pragmatic Programmer.pdf',
	'lab.md',
	'Launch.md',
];

test('file names outrank folder names and exact names win', () => {
	expect(quickOpen(paths, 'launch').map((match) => match.path)).toEqual([
		'Launch.md',
		'Projects/Launch brief.md',
		'Archive/launch-notes-2023.md',
	]);
});

test('letters match in order across words', () => {
	expect(quickOpen(paths, 'lb')[0]?.path).toBe('Projects/Launch brief.md');
	expect(quickOpen(paths, 'prag prog')[0]?.path).toBe(
		'Reading/Books/The Pragmatic Programmer.pdf',
	);
	expect(quickOpen(paths, 'books')[0]?.path).toBe(
		'Reading/Books/The Pragmatic Programmer.pdf',
	);
});

test('no match and the result limit', () => {
	expect(quickOpenScore('lab.md', 'xyz')).toBeNull();
	expect(quickOpen(paths, 'a', 2)).toHaveLength(2);
	expect(quickOpen([], 'a')).toEqual([]);
});
