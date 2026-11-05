import { beforeAll, expect, test } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import { createLiveMarkdownEditor } from './factory';
import type { FindController, FindHost } from './find';

beforeAll(() => {
	if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register();
});

function editorWithFind(text: string) {
	const events: string[] = [];
	let controller: FindController | null = null;
	const host: FindHost = {
		open: (value) => {
			controller = value;
			events.push('open');
		},
		focus: () => events.push('focus'),
		close: () => {
			controller = null;
			events.push('close');
		},
	};
	const parent = document.createElement('div');
	document.body.append(parent);
	const editor = createLiveMarkdownEditor(parent, { text, find: host });
	return { editor, events, controller: () => controller };
}

test('find opens through the host and counts and steps through matches', () => {
	const { editor, events, controller } = editorWithFind(
		'beta alpha beta gamma beta',
	);
	editor.find();
	expect(events).toEqual(['open']);
	const find = controller()!;

	find.setQuery('beta');
	expect(find.query()).toBe('beta');
	expect(find.matches()).toEqual({ current: 1, total: 3, capped: false });

	find.next();
	expect(find.matches().current).toBe(2);
	find.next();
	find.next();
	expect(find.matches().current).toBe(1);
	find.previous();
	expect(find.matches().current).toBe(3);

	find.setQuery('delta');
	expect(find.matches()).toEqual({ current: 0, total: 0, capped: false });

	// Asking again while open refocuses the bar instead of reopening it.
	editor.find();
	expect(events).toEqual(['open', 'focus']);

	find.close();
	expect(events).toEqual(['open', 'focus', 'close']);
	expect(controller()).toBeNull();
	editor.destroy();
});

test('typing a query selects the nearest match at or after the caret', () => {
	const { editor, controller } = editorWithFind('cat dog cat dog');
	editor.view.dispatch({ selection: { anchor: 5 } });
	editor.find();
	controller()!.setQuery('cat');
	const { from, to } = editor.view.state.selection.main;
	expect([from, to]).toEqual([8, 11]);
	editor.destroy();
});

test('queries are literal text, not patterns', () => {
	const { editor, controller } = editorWithFind('a.b axb a.b');
	editor.find();
	controller()!.setQuery('a.b');
	expect(controller()!.matches().total).toBe(2);
	editor.destroy();
});
