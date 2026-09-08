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

describe('PDF previews', () => {
	test('both syntaxes mount app-owned viewers and preserve source bytes', async () => {
		const source =
			'![[lecture.pdf#page=7]]\n\n![](lecture.pdf#page=9)\n\n[Read](lecture.pdf#page=11)\n\nInline ![[lecture.pdf#page=3]] end\n';
		const originalObserver = globalThis.IntersectionObserver;
		const visibility: IntersectionObserverCallback[] = [];
		globalThis.IntersectionObserver = class {
			constructor(callback: IntersectionObserverCallback) {
				visibility.push(callback);
			}
			observe() {}
			disconnect() {}
			unobserve() {}
			takeRecords() {
				return [];
			}
		} as unknown as typeof IntersectionObserver;
		const host = document.createElement('div');
		document.body.append(host);
		const documentModel = createLiveMarkdownDocument('markdown', source);
		let mounts = 0;
		let cleanups = 0;
		const opened: number[] = [];
		const editor = createLiveMarkdownEditor(host, {
			ytext: documentModel.ytext,
			readOnly: true,
			resolveLink: async (target) => ({
				kind: 'pdf',
				relativePath: 'lecture.pdf',
				page: Number(target.split('page=')[1]),
			}),
			openPdf: (target) => {
				opened.push(target.page ?? 1);
			},
			mountPdfEmbed: (container) => {
				mounts++;
				container.textContent = 'App PDF viewer';
				return () => {
					cleanups++;
				};
			},
		});
		await new Promise((resolve) => setTimeout(resolve, 30));
		expect(mounts).toBe(0);
		for (const callback of visibility)
			callback(
				[{ isIntersecting: true } as IntersectionObserverEntry],
				{} as IntersectionObserver,
			);
		expect(mounts).toBe(2);
		for (const callback of visibility)
			callback(
				[{ isIntersecting: false } as IntersectionObserverEntry],
				{} as IntersectionObserver,
			);
		expect(cleanups).toBe(2);
		for (const callback of visibility)
			callback(
				[{ isIntersecting: true } as IntersectionObserverEntry],
				{} as IntersectionObserver,
			);
		expect(host.querySelectorAll('.cm-pdf-preview').length).toBe(4);
		expect(host.querySelectorAll('.cm-pdf-preview > div').length).toBe(2);
		const read = Array.from(host.querySelectorAll('button')).find(
			(button) => button.textContent === 'Read',
		);
		read?.click();
		expect(opened).toEqual([11]);
		expect(editor.doc()).toBe(source);
		editor.destroy();
		documentModel.destroy();
		host.remove();
		expect(cleanups).toBe(4);
		globalThis.IntersectionObserver = originalObserver;
	});
});
