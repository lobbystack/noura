import { expect, test } from 'bun:test';
import { analyzeMarkdownSafety, detectSuspiciousShrink } from './index';

test('unsupported Markdown requests source mode instead of lossy conversion', () => {
	expect(
		analyzeMarkdownSafety('# Title\n\n<div>kept</div>').requiresSourceMode,
	).toBe(true);
});
test('shrink guard detects large accidental document loss', () => {
	expect(detectSuspiciousShrink('x'.repeat(1000), 'x'.repeat(100))).toBe(true);
});
