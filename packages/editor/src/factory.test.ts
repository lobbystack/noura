import { beforeAll, describe, expect, test } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import { EditorView } from '@codemirror/view';
import { forceParsing, syntaxTree } from '@codemirror/language';
import type { Decoration } from '@codemirror/view';
import { createLiveMarkdownEditor, markdownBlockStyle } from './factory';
import type { LiveMarkdownOptions } from './types';

beforeAll(() => {
	if (typeof document === 'undefined') GlobalRegistrator.register();
});

function mount(text: string, options: Partial<LiveMarkdownOptions> = {}) {
	const parent = document.createElement('div');
	document.body.appendChild(parent);
	const editor = createLiveMarkdownEditor(parent, { text, ...options });
	return {
		editor,
		dispose() {
			editor.destroy();
			parent.remove();
		},
	};
}

function type(view: EditorView, at: number, text: string) {
	view.dispatch({
		changes: { from: at, insert: text },
		selection: { anchor: at + text.length },
		userEvent: 'input.type',
	});
}

function decorationClasses(view: EditorView, from: number) {
	const classes: string[] = [];
	for (const input of view.state.facet(EditorView.decorations)) {
		if (typeof input === 'function') continue;
		input.between(
			from,
			view.state.doc.length,
			(start, _end, deco: Decoration) => {
				const name = (deco.spec as { class?: string }).class;
				if (start >= from && name) classes.push(name);
			},
		);
	}
	return classes;
}

describe('canonical text updates', () => {
	test('keep the caret where it was instead of jumping to the start', () => {
		const { editor, dispose } = mount('line one\nline two\nline three\n');
		editor.view.dispatch({ selection: { anchor: 15 } });
		editor.setText('line one\nline two\nline three\nexternal\n');
		expect(editor.view.state.selection.main.head).toBe(15);
		editor.setText(
			'new first line\nline one\nline two\nline three\nexternal\n',
		);
		expect(editor.view.state.selection.main.head).toBe(15 + 15);
		dispose();
	});

	test('keep earlier edits undoable and never undo the canonical change', () => {
		const { editor, dispose } = mount('alpha\n');
		type(editor.view, 5, ' beta');
		// An external edit appends a line.
		editor.setText('alpha beta\nfrom outside\n');
		editor.undo();
		expect(editor.doc()).toBe('alpha\nfrom outside\n');
		editor.redo();
		expect(editor.doc()).toBe('alpha beta\nfrom outside\n');
		dispose();
	});

	test('do not report canonical changes as user edits', () => {
		let edits = 0;
		const { editor, dispose } = mount('text', { onChange: () => (edits += 1) });
		editor.setText('changed text');
		expect(edits).toBe(0);
		type(editor.view, 0, 'x');
		expect(edits).toBe(1);
		dispose();
	});

	test('rebase keeps text typed after the base and applies the other side', () => {
		const { editor, dispose } = mount('one\ntwo\n');
		// The editor showed "one\ntwo\n" when a save started; the user kept
		// typing, and the save came back merged with an external edit.
		type(editor.view, 7, ' more');
		editor.rebase('one\ntwo\n', 'zero\none\ntwo\n');
		expect(editor.doc()).toBe('zero\none\ntwo more\n');
		expect(editor.view.state.selection.main.head).toBe(17);
		dispose();
	});
});

describe('line endings', () => {
	test('a lone carriage return stays one character and round-trips', () => {
		const text = 'a\rb\nc\r\n';
		const { editor, dispose } = mount(text);
		expect(editor.doc()).toBe(text);
		expect(editor.view.state.doc.length).toBe(text.length);
		expect(editor.view.state.doc.lines).toBe(3);
		dispose();
	});
});

describe('live preview on long documents', () => {
	test('styles headings the background parser reaches after mount', () => {
		let text = '';
		for (let index = 0; index < 300; index += 1)
			text += `## Heading ${index}\n\nParagraph ${index} text\n\n`;
		const { editor, dispose } = mount(text);
		const parsed = syntaxTree(editor.view.state).length;
		const headingsAfter = () =>
			decorationClasses(editor.view, parsed + 1).filter((name) =>
				name.includes('cm-md-h2'),
			).length;
		if (parsed < text.length) expect(headingsAfter()).toBe(0);
		// The background parser finishes in a transaction of its own; the
		// preview must follow it without waiting for an edit.
		forceParsing(editor.view, text.length, 5000);
		expect(syntaxTree(editor.view.state).length).toBe(text.length);
		expect(headingsAfter()).toBeGreaterThan(0);
		dispose();
	});
});

describe('markdownBlockStyle', () => {
	test('reports the heading level at the selection line', () => {
		expect(markdownBlockStyle('plain text')).toBe('text');
		expect(markdownBlockStyle('# First')).toBe('heading1');
		expect(markdownBlockStyle('## Second')).toBe('heading2');
		expect(markdownBlockStyle('### Third')).toBe('heading3');
	});
});
