import { beforeAll, describe, expect, test } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import { createLiveMarkdownEditor } from './factory';
import type { LiveMarkdownOptions } from './types';

beforeAll(() => {
	if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register();
});

const KITCHEN_SINK = [
	'---',
	'title: Kitchen sink',
	'tags: [a, b]',
	'---',
	'',
	'# Heading with **bold** and *italic*',
	'',
	'> [!note] Callout title',
	'> Callout body with [[Link|alias]] and #tag',
	'',
	'> quote line one',
	'> quote line two',
	'',
	'- [ ] open task',
	'* [x] done task',
	'1. [ ] ordered task',
	'- [ ]',
	'   1. nested',
	'',
	'```ts',
	'const answer: number = 42; // comment',
	'```',
	'',
	'$$',
	'x^2 + y^2 = z^2',
	'$$',
	'',
	'Inline $a+b$ and `code` and ==highlight== and %%hidden%% and <u>u</u>.',
	'',
	'![Alt](pic.png "Title") ![[photo.jpg|200]] ![[Other note]]',
	'',
	'[a link](https://example.com) and https://bare.example.com',
	'',
	'| A | B |',
	'| - | - |',
	'| 1 | 2 |',
	'',
	'Setext',
	'===',
	'',
	'---',
	'',
	'Last line ^block-id',
	'',
].join('\n');

function mount(text: string, options: Partial<LiveMarkdownOptions> = {}) {
	const host = document.createElement('div');
	document.body.append(host);
	const editor = createLiveMarkdownEditor(host, { text, ...options });
	return {
		host,
		editor,
		dispose() {
			editor.destroy();
			host.remove();
		},
	};
}

describe('live preview in a mounted editor', () => {
	test('every caret position renders without errors and keeps the text', () => {
		const { editor, dispose } = mount(KITCHEN_SINK, {
			linkSuggestions: () => [{ label: 'Other note', target: 'Other note' }],
			openLink: () => {},
		});
		try {
			for (let position = 0; position <= KITCHEN_SINK.length; position += 1) {
				editor.view.dispatch({ selection: { anchor: position } });
			}
			expect(editor.doc()).toBe(KITCHEN_SINK);
			for (let position = KITCHEN_SINK.length; position >= 0; position -= 37) {
				editor.view.dispatch({
					changes: { from: position, insert: 'x' },
					selection: { anchor: position + 1 },
				});
				editor.view.dispatch({
					changes: { from: position, to: position + 1 },
				});
			}
			expect(editor.doc()).toBe(KITCHEN_SINK);
		} finally {
			dispose();
		}
	});

	test('clicking a rendered checkbox toggles its task', () => {
		const { host, editor, dispose } = mount('intro\n\n- [ ] task\n');
		try {
			const box = host.querySelector<HTMLElement>('.cm-md-checkbox');
			expect(box).not.toBeNull();
			box?.dispatchEvent(
				new MouseEvent('mousedown', { bubbles: true, button: 0 }),
			);
			expect(editor.doc()).toBe('intro\n\n- [x] task\n');
		} finally {
			dispose();
		}
	});

	test('rendered links follow on click', () => {
		const opened: string[] = [];
		const { host, dispose } = mount(
			'intro\n\nSee [[Plan|the plan]] and [docs](https://example.com)\n',
			{ openLink: (target) => opened.push(target) },
		);
		try {
			host
				.querySelector<HTMLElement>('.cm-md-link-preview')
				?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
			host
				.querySelector<HTMLElement>('.cm-md-link-rendered')
				?.dispatchEvent(
					new MouseEvent('mousedown', { bubbles: true, button: 0 }),
				);
			expect(opened).toEqual(['Plan', 'https://example.com']);
		} finally {
			dispose();
		}
	});
});
