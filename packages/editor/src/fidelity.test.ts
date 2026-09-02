import { describe, expect, test } from 'bun:test';
import {
	composeRawText,
	detectSuspiciousShrink,
	preserveLineMetadata,
} from './fidelity';

describe('preserveLineMetadata', () => {
	test('normalizes CRLF and BOM and restores both', () => {
		const raw = '\uFEFFfirst\r\nsecond\r\n';
		const { text, metadata } = preserveLineMetadata(raw);
		expect(text).toBe('first\nsecond\n');
		expect(metadata.usesCrlf).toBe(true);
		expect(metadata.hasBom).toBe(true);
		expect(composeRawText(text, metadata)).toBe(raw);
	});

	test('plain files round-trip unchanged', () => {
		const raw = 'one\ntwo\n';
		const { text, metadata } = preserveLineMetadata(raw);
		expect(text).toBe(raw);
		expect(metadata.usesCrlf).toBe(false);
		expect(metadata.hasBom).toBe(false);
		expect(composeRawText(text, metadata)).toBe(raw);
	});

	for (const [name, raw] of [
		['tables', '| A | B |\n| - | - |\n| 1 | 2 |\n'],
		['math', '$x^2$\n\n$$y = mx + b$$\n'],
		['obsidian syntax', '[[Note|Alias]] ![[image.png]] #tag ^block\n'],
		['html and comments', '<custom value="1">text</custom>\n%%hidden%%\n'],
		['unicode', '# Café ☕️\n\nこんにちは\n'],
		['missing final newline', 'last line'],
	] as const) {
		test(`${name} remains exact`, () => {
			const { text, metadata } = preserveLineMetadata(raw);
			expect(composeRawText(text, metadata)).toBe(raw);
		});
	}
});

describe('detectSuspiciousShrink', () => {
	test('flags large unexplained collapses', () => {
		const before = 'x'.repeat(1000);
		expect(detectSuspiciousShrink(before, 'x'.repeat(100))).toBe(true);
		expect(detectSuspiciousShrink(before, before)).toBe(false);
	});
});
