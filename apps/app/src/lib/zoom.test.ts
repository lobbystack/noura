import { expect, test } from 'bun:test';
import { nextZoom, ZOOM_LEVELS } from './zoom';

test('zoom steps through the levels and stops at either end', () => {
	expect(nextZoom(1, 1)).toBe(1.1);
	expect(nextZoom(1, -1)).toBe(0.9);
	expect(nextZoom(ZOOM_LEVELS[0], -1)).toBe(ZOOM_LEVELS[0]);
	expect(nextZoom(2, 1)).toBe(2);
	// An unknown level starts from actual size.
	expect(nextZoom(1.05, 1)).toBe(1.1);
});
