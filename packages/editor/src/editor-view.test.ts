import { describe, expect, test, beforeAll } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import type { Decoration } from '@codemirror/view';
import { EditorView } from '@codemirror/view';
import {
	createLiveMarkdownDocument,
	createLiveMarkdownEditor,
} from './factory';

// Registration stays for the process lifetime: widgets render
// asynchronously (KaTeX), and later callbacks must still see a DOM.
beforeAll(() => {
	GlobalRegistrator.register();
});

const SAMPLE = [
	'# Plan',
	'',
	'| A | B |',
	'| - | - |',
	'| 1 | 2 |',
	'',
	'$$E = mc^2$$',
	'',
	'![[Notes/index]]',
	'',
	'Inline `[[Note]]` code',
	'',
].join('\n');

interface DecoratedRange {
	from: number;
	to: number;
	deco: Decoration;
}

function collectBlockPreviewRanges(view: EditorView): DecoratedRange[] {
	const ranges: DecoratedRange[] = [];
	for (const input of view.state.facet(EditorView.decorations)) {
		// A state field supplies the set directly; function inputs would be
		// the plugin path that rejects block decorations.
		if (typeof input === 'function') continue;
		input.between(
			0,
			view.state.doc.length,
			(from: number, to: number, deco: Decoration) => {
				ranges.push({ from, to, deco });
			},
		);
	}
	return ranges;
}

describe('live markdown editor view', () => {
	test('mounts with block previews for tables, display math, and embeds', () => {
		// CodeMirror rejects block decorations supplied through a view plugin
		// with an unconditional RangeError during the first draw. This mount
		// is the regression test for the state-field delivery path.
		const parent = document.createElement('div');
		document.body.appendChild(parent);
		const live = createLiveMarkdownDocument('markdown', SAMPLE);
		const editor = createLiveMarkdownEditor(parent, { ytext: live.ytext });
		try {
			const ranges = collectBlockPreviewRanges(editor.view);
			const blocks = ranges.filter((item) => item.deco.spec.block === true);

			const tableStartLine = editor.view.state.doc.line(3);
			const tableEndLine = editor.view.state.doc.line(5);
			const mathLine = editor.view.state.doc.line(7);
			const embedLine = editor.view.state.doc.line(9);
			expect(blocks).toContainEqual(
				expect.objectContaining({
					from: tableStartLine.from,
					to: tableEndLine.to,
				}),
			);
			expect(blocks).toContainEqual(
				expect.objectContaining({
					from: mathLine.from,
					to: mathLine.to,
				}),
			);
			expect(blocks).toContainEqual(
				expect.objectContaining({
					from: embedLine.from,
					to: embedLine.to,
				}),
			);
			// Code-span content stays literal: no wikilink decoration inside it.
			expect(
				ranges.some((item) => item.deco.spec.class === 'cm-md-wikilink'),
			).toBe(false);
		} finally {
			editor.destroy();
			live.destroy();
			parent.remove();
		}
	});
});
