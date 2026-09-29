import { expect, test } from 'bun:test';
import {
	shortcutCommand,
	shortcutOnce,
	type ShortcutEvent,
} from './app-shortcuts';

test('a menu report of the same press runs once; repeated presses all run', () => {
	const once = shortcutOnce(300);
	expect(once('close-tab', 'key', 0)).toBe(true);
	expect(once('close-tab', 'menu', 10)).toBe(false);
	expect(once('close-tab', 'key', 100)).toBe(true);
	expect(once('close-tab', 'key', 150)).toBe(true);
	expect(once('close-tab', 'menu', 500)).toBe(true);
	expect(once('close-tab', 'key', 520)).toBe(false);
	expect(once('new-note', 'menu', 530)).toBe(true);
});

function press(key: string, extra: Partial<ShortcutEvent> = {}): ShortcutEvent {
	return {
		key,
		metaKey: false,
		ctrlKey: false,
		shiftKey: false,
		altKey: false,
		...extra,
	};
}

test('Command on a Mac, Control elsewhere', () => {
	expect(shortcutCommand(press('n', { metaKey: true }), 'mac')).toBe(
		'new-note',
	);
	expect(shortcutCommand(press('n', { ctrlKey: true }), 'mac')).toBeNull();
	expect(shortcutCommand(press('n', { ctrlKey: true }), 'windows')).toBe(
		'new-note',
	);
	expect(shortcutCommand(press('n', { metaKey: true }), 'linux')).toBeNull();
});

test('every command has its key', () => {
	const mac = (key: string, extra: Partial<ShortcutEvent> = {}) =>
		shortcutCommand(press(key, { metaKey: true, ...extra }), 'mac');
	expect(mac('N', { shiftKey: true })).toBe('new-folder');
	expect(mac('o')).toBe('quick-open');
	expect(mac('F', { shiftKey: true })).toBe('search');
	expect(mac('w')).toBe('close-tab');
	expect(mac('\\')).toBe('toggle-sidebar');
	expect(mac('1')).toBe('go-home');
	expect(mac('6')).toBe('go-ai');
	expect(mac('7')).toBeNull();
	expect(mac('}', { shiftKey: true, code: 'BracketRight' })).toBe('next-tab');
	expect(mac('{', { shiftKey: true, code: 'BracketLeft' })).toBe(
		'previous-tab',
	);
	expect(shortcutCommand(press('Tab', { ctrlKey: true }), 'mac')).toBe(
		'next-tab',
	);
	expect(
		shortcutCommand(press('Tab', { ctrlKey: true, shiftKey: true }), 'linux'),
	).toBe('previous-tab');
	expect(mac('n', { altKey: true })).toBeNull();
	expect(shortcutCommand(press('n'), 'mac')).toBeNull();
});
