import { describe, expect, test } from 'bun:test';
import { normalizeFileName, renamedPath, splitFileName } from './rename';

describe('splitFileName', () => {
	test('separates folder, name, and extension', () => {
		expect(splitFileName('Projects/Plan.md')).toEqual({
			folder: 'Projects',
			stem: 'Plan',
			extension: '.md',
		});
		expect(splitFileName('notes.v2.txt')).toEqual({
			folder: '',
			stem: 'notes.v2',
			extension: '.txt',
		});
		expect(splitFileName('Makefile')).toEqual({
			folder: '',
			stem: 'Makefile',
			extension: '',
		});
	});
});

describe('normalizeFileName', () => {
	test('keeps what was typed apart from characters files cannot hold', () => {
		expect(normalizeFileName('  Weekly plan  ')).toBe('Weekly plan');
		expect(normalizeFileName('a/b\\c:d*e?f"g<h>i|j')).toBe('abcdefghij');
		expect(normalizeFileName('..hidden')).toBe('hidden');
		expect(normalizeFileName('   ')).toBe('');
	});
});

describe('renamedPath', () => {
	test('keeps the folder and extension', () => {
		expect(renamedPath('Projects/Plan.md', 'Roadmap ')).toBe(
			'Projects/Roadmap.md',
		);
		expect(renamedPath('todo.txt', 'done')).toBe('done.txt');
	});

	test('returns null when the name is empty or unchanged', () => {
		expect(renamedPath('Plan.md', '')).toBeNull();
		expect(renamedPath('Plan.md', ' Plan ')).toBeNull();
	});

	test('allows a case-only rename', () => {
		expect(renamedPath('plan.md', 'Plan')).toBe('Plan.md');
	});
});
