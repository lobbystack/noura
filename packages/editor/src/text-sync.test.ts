import { describe, expect, test } from 'bun:test';
import { changesBetween, rebaseText, textOf } from './text-sync';

describe('changesBetween', () => {
	test('touches only the changed ranges', () => {
		const before = 'one\ntwo\nthree\n';
		const after = 'one\n2\nthree\nfour\n';
		const changes = changesBetween(before, after);
		expect(changes.apply(textOf(before)).toString()).toBe(after);
		// The first line is untouched, so a caret there stays put.
		expect(changes.mapPos(2)).toBe(2);
	});

	test('keeps lone carriage returns as characters', () => {
		const before = 'a\rb';
		const after = 'a\rbc';
		expect(changesBetween(before, after).apply(textOf(before)).toString()).toBe(
			after,
		);
	});
});

describe('rebaseText', () => {
	test('keeps keystrokes typed after the base and applies the merged side', () => {
		// A save submitted "Hello wor" and came back merged with an external
		// line; meanwhile the user typed "ld".
		expect(
			rebaseText('Hello wor\n', 'Hello world\n', 'Title\nHello wor\n'),
		).toBe('Title\nHello world\n');
	});

	test('never drops local text inside a range the other side deleted', () => {
		const result = rebaseText('a b c\n', 'a bXY c\n', 'a c\n');
		expect(result).toContain('XY');
	});

	test('returns the target when nothing was typed and the local text otherwise', () => {
		expect(rebaseText('same', 'same', 'other')).toBe('other');
		expect(rebaseText('same', 'mine', 'same')).toBe('mine');
	});
});
