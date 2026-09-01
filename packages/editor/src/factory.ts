import { history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { GFM } from '@lezer/markdown';
import {
	Decoration,
	EditorView,
	ViewPlugin,
	WidgetType,
	keymap,
} from '@codemirror/view';
import type { DecorationSet } from '@codemirror/view';
import { EditorState, type Extension } from '@codemirror/state';
import { yCollab } from 'y-codemirror.next';
import * as Y from 'yjs';
import { buildDecorations } from './preview';
import { formattingCommands, toggleCheckboxes } from './commands';

export interface LiveMarkdownOptions {
	/** The shared Y.Text holding the canonical Markdown body. */
	ytext: Y.Text;
	/** Whether editor history and selection walk the Yjs document. */
	collaborative?: boolean;
	/** Resolve a local image path to a rendered URL (data URI). */
	resolveImage?: (src: string) => string | null | Promise<string | null>;
	/** Called after each document edit and selection change. */
	onChange?: (view: EditorView) => void;
}

export interface LiveMarkdownEditor {
	view: EditorView;
	doc: () => string;
	setText: (text: string) => void;
	undo: () => void;
	redo: () => void;
	destroy: () => void;
	focus: () => void;
}

export interface LiveMarkdownDocument {
	ytext: Y.Text;
	destroy: () => void;
}

/**
 * Owns the Yjs document lifecycle behind the editor package boundary.
 *
 * Yjs 13.6.32 implements `Doc.destroy()` at runtime but omits it from the
 * published `Doc.d.ts`. Keep that upstream typing defect isolated here rather
 * than leaking structural casts into every Svelte editor surface.
 */
export function createLiveMarkdownDocument(
	name: string,
	initialText = '',
): LiveMarkdownDocument {
	const doc = new Y.Doc();
	const ytext = doc.getText(name);
	if (initialText) ytext.insert(0, initialText);
	return {
		ytext,
		destroy: () => {
			(doc as Y.Doc & { destroy(): void }).destroy();
		},
	};
}

class ImageWidget extends WidgetType {
	constructor(
		private readonly alt: string,
		private readonly src: string,
		private readonly resolve?: (
			src: string,
		) => string | null | Promise<string | null>,
	) {
		super();
	}

	eq(widget: ImageWidget) {
		return widget.src === this.src && widget.alt === this.alt;
	}

	toDOM() {
		const wrap = document.createElement('span');
		wrap.className = 'cm-md-image';
		wrap.setAttribute('aria-label', this.alt || 'image');
		const placeholder = document.createElement('span');
		placeholder.className = 'cm-md-image-placeholder';
		placeholder.textContent = 'Image: ' + this.src;
		wrap.appendChild(placeholder);
		const resolved = this.resolve?.(this.src) ?? null;
		const apply = (url: string | null) => {
			if (!url) return;
			const img = document.createElement('img');
			img.src = url;
			img.alt = this.alt;
			img.loading = 'lazy';
			wrap.replaceChildren(img);
		};
		if (resolved instanceof Promise) {
			void resolved.then(apply);
		} else if (resolved) {
			apply(resolved);
		}
		return wrap;
	}

	ignoreEvent() {
		return false;
	}
}

const checkboxClickPlugin = ViewPlugin.fromClass(
	class {
		constructor(readonly view: EditorView) {}
	},
	{
		eventHandlers: {
			mousedown(event: MouseEvent, view: EditorView) {
				const target = event.target as HTMLElement | null;
				if (!target?.classList.contains('cm-md-checkbox')) return false;
				event.preventDefault();
				const pos = view.posAtDOM(target);
				const line = view.state.doc.lineAt(pos);
				toggleCheckboxes(view, [line.number]);
				return true;
			},
		},
	},
);

const previewPlugin = ViewPlugin.fromClass(
	class {
		decorations: DecorationSet;
		constructor(
			readonly view: EditorView,
			private specs: { resolveImage?: LiveMarkdownOptions['resolveImage'] },
		) {
			this.decorations = this.build();
		}
		update(update: {
			docChanged: boolean;
			selectionSet: boolean;
			viewportChanged: boolean;
		}) {
			if (update.docChanged || update.selectionSet || update.viewportChanged) {
				this.decorations = this.build();
			}
		}
		private build(): DecorationSet {
			return buildDecorations(
				this.view,
				Decoration,
				(alt, src) => new ImageWidget(alt, src, this.specs.resolveImage),
			);
		}
	},
	{
		decorations: (plugin) => plugin.decorations,
	},
);

const checkboxMarkDecoration = Decoration.replace({
	widget: new (class extends WidgetType {
		eq() {
			return true;
		}
		toDOM() {
			const box = document.createElement('span');
			box.className = 'cm-md-checkbox';
			box.setAttribute('role', 'checkbox');
			box.setAttribute('aria-checked', 'false');
			box.contentEditable = 'false';
			return box;
		}
		ignoreEvent() {
			return false;
		}
	})(),
});

export function createLiveMarkdownEditor(
	parent: HTMLElement,
	options: LiveMarkdownOptions,
): LiveMarkdownEditor {
	const doc = options.ytext;
	const extensions: Extension[] = [
		markdown({ base: markdownLanguage, extensions: [GFM] }),
		EditorView.lineWrapping,
		previewPlugin.of({ resolveImage: options.resolveImage }),
		checkboxClickPlugin,
		history(),
		keymap.of([...historyKeymap, indentWithTab]),
		EditorView.theme({
			'&': { backgroundColor: 'transparent', height: '100%' },
			'.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.75' },
			'.cm-content': { caretColor: 'var(--foreground)' },
			'.cm-md-heading': { fontWeight: '600' },
			'.cm-md-h1': { fontSize: '2rem', padding: '0.6rem 0' },
			'.cm-md-h2': { fontSize: '1.6rem', padding: '0.5rem 0' },
			'.cm-md-h3': { fontSize: '1.3rem', padding: '0.4rem 0' },
			'.cm-md-strong': { fontWeight: '700' },
			'.cm-md-emphasis': { fontStyle: 'italic' },
			'.cm-md-strike': { textDecoration: 'line-through' },
			'.cm-md-code': {},
			'.cm-md-url': { color: 'var(--muted-foreground)' },
			'.cm-md-list-mark': { color: 'var(--muted-foreground)' },
			'.cm-md-table-row': {},
		}),
	];
	let undoManager: Y.UndoManager | null = null;
	if (options.collaborative !== false) {
		undoManager = new Y.UndoManager(options.ytext);
		extensions.push(yCollab(options.ytext, null, { undoManager }));
	} else {
		extensions.push(history(), keymap.of([...historyKeymap]));
	}
	const view = new EditorView({
		state: EditorState.create({ doc: doc.toString(), extensions }),
		parent,
	});
	const listener = () => {
		options.onChange?.(view);
	};
	view.dom.addEventListener('input', listener);
	return {
		view,
		doc: () => view.state.doc.toString(),
		setText: (text: string) => {
			const current = view.state.doc.toString();
			if (current === text) return;
			if (options.collaborative === false) {
				view.dispatch({
					changes: { from: 0, to: view.state.doc.length, insert: text },
				});
				return;
			}
			doc.doc?.transact(() => {
				doc.delete(0, doc.length);
				doc.insert(0, text);
			});
		},
		undo: () => undoManager?.undo(),
		redo: () => undoManager?.redo(),
		focus: () => view.focus(),
		destroy: () => {
			view.dom.removeEventListener('input', listener);
			view.destroy();
		},
	};
}
