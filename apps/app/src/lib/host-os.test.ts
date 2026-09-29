import { expect, test } from 'bun:test';
import { hostOs, revealLabel, shortcutKeys, shortcutLabel } from './host-os';

test('desktop targets decide the platform before the user agent', () => {
	expect(hostOs('darwin', 'Windows')).toBe('mac');
	expect(hostOs('windows', 'Macintosh')).toBe('windows');
	expect(hostOs('linux', 'Macintosh')).toBe('linux');
	expect(hostOs('', 'Mozilla/5.0 (Macintosh; Intel Mac OS X)')).toBe('mac');
	expect(hostOs('', 'Mozilla/5.0 (Windows NT 10.0)')).toBe('windows');
	expect(hostOs('', 'Mozilla/5.0 (X11; Linux x86_64)')).toBe('linux');
});

test('labels follow the platform', () => {
	expect(revealLabel('mac')).toBe('Reveal in Finder');
	expect(revealLabel('windows')).toBe('Show in Explorer');
	expect(revealLabel('linux')).toBe('Show in file manager');
	expect(shortcutKeys(['Mod', 'Shift', 'F'], 'mac')).toEqual(['⌘', '⇧', 'F']);
	expect(shortcutKeys(['Mod', 'Shift', 'F'], 'linux')).toEqual([
		'Ctrl',
		'Shift',
		'F',
	]);
	expect(shortcutLabel(['Mod', 'K'], 'mac')).toBe('⌘K');
	expect(shortcutLabel(['Mod', 'K'], 'windows')).toBe('Ctrl+K');
});
