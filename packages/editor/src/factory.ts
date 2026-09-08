import { PdfWidget } from './pdf-widget';
import { isPdfTarget } from './pdf-target';
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
import {
	Annotation,
	EditorState,
	StateEffect,
	StateField,
	Transaction,
	type Extension,
	type TransactionSpec,
} from '@codemirror/state';
import { yCollab, yUndoManagerKeymap } from 'y-codemirror.next';
import * as Y from 'yjs';
import 'katex/dist/katex.min.css';
import { buildDecorations, type PreviewViewportRange } from './preview';
import { formattingCommands, toggleCheckboxes } from './commands';
import type {
	LiveMarkdownDocument,
	LiveMarkdownEditor,
	LiveMarkdownOptions,
	MarkdownBlockStyle,
	MarkdownFormat,
} from './types';

const canonicalUpdate = Annotation.define<boolean>();

export function markdownBlockStyle(line: string): MarkdownBlockStyle {
	if (/^#\s/.test(line)) return 'heading1';
	if (/^##\s/.test(line)) return 'heading2';
	if (/^###\s/.test(line)) return 'heading3';
	return 'text';
}

interface CanonicalTextView {
	readonly state: EditorState;
	dispatch(spec: TransactionSpec): void;
}

/** @internal Exported for the DOM-free canonical reload regression test. */
export function applyCanonicalText(
	view: CanonicalTextView,
	text: string,
	undoManager: Y.UndoManager | null,
) {
	if (view.state.doc.toString() === text) return;
	view.dispatch({
		changes: { from: 0, to: view.state.doc.length, insert: text },
		annotations: [canonicalUpdate.of(true), Transaction.addToHistory.of(false)],
	});
	// The Yjs binding tracks ordinary CodeMirror transactions as local edits.
	// A canonical reload is a new baseline, so retaining the earlier undo stack
	// would let Undo restore and autosave stale pre-reconciliation content.
	undoManager?.clear();
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
		const remote = /^https?:\/\//i.test(this.src);
		if (remote) {
			const button = document.createElement('button');
			button.type = 'button';
			button.className = 'cm-md-image-load';
			button.textContent = 'Load remote image';
			button.addEventListener('click', () => {
				const img = document.createElement('img');
				img.src = this.src;
				img.alt = this.alt;
				img.loading = 'lazy';
				wrap.replaceChildren(img);
			});
			wrap.appendChild(button);
			return wrap;
		}
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

class MathWidget extends WidgetType {
	constructor(
		private readonly source: string,
		private readonly displayMode: boolean,
	) {
		super();
	}

	eq(widget: MathWidget) {
		return (
			widget.source === this.source && widget.displayMode === this.displayMode
		);
	}

	toDOM() {
		const element = document.createElement(this.displayMode ? 'div' : 'span');
		element.className = this.displayMode
			? 'cm-md-math-block'
			: 'cm-md-math-inline';
		// The raw source stays visible until (and if) KaTeX fails to load or
		// render; a math failure must never reject unhandled.
		element.textContent = this.source;
		void import('katex')
			.then(({ default: katex }) => {
				katex.render(this.source, element, {
					displayMode: this.displayMode,
					throwOnError: false,
					trust: false,
					strict: 'ignore',
				});
			})
			.catch(() => {
				/* keep the plain-text source */
			});
		return element;
	}
}

class LinkWidget extends WidgetType {
	constructor(
		private readonly target: string,
		private readonly label: string,
		private readonly embed: boolean,
		private readonly from: number,
		private readonly to: number,
		private readonly resolve?: LiveMarkdownOptions['resolveLink'],
	) {
		super();
	}

	eq(widget: LinkWidget) {
		return (
			widget.target === this.target &&
			widget.label === this.label &&
			widget.embed === this.embed &&
			widget.from === this.from &&
			widget.to === this.to
		);
	}

	// Decorations built in a state field run without a view; the view only
	// exists once the rendered DOM is wired to event handlers.
	toDOM(view: EditorView) {
		const element = document.createElement(this.embed ? 'div' : 'span');
		element.className = this.embed
			? 'cm-md-embed-preview'
			: 'cm-md-link-preview';
		element.tabIndex = 0;
		element.textContent = this.label;
		const revealSource = () => {
			view.dispatch({ selection: { anchor: this.from, head: this.to } });
			view.focus();
		};
		element.addEventListener('click', revealSource);
		element.addEventListener('keydown', (event) => {
			const keyboardEvent = event as KeyboardEvent;
			if (keyboardEvent.key === 'Enter' || keyboardEvent.key === ' ') {
				event.preventDefault();
				revealSource();
			}
		});
		if (!this.resolve) return element;
		void this.resolve(this.target)
			.then((resolved) => {
				if (resolved.kind === 'unresolved') {
					element.dataset.state = 'unresolved';
					return;
				}
				element.dataset.state = 'resolved';
				if (!this.embed) {
					element.textContent = resolved.label ?? this.label;
					return;
				}
				const heading = document.createElement('strong');
				heading.textContent = resolved.label ?? this.label;
				const preview = document.createElement('p');
				preview.textContent = (resolved.preview ?? '').slice(0, 800);
				element.replaceChildren(heading, preview);
			})
			.catch(() => {
				element.dataset.state = 'unresolved';
			});
		return element;
	}
}

interface TableCell {
	text: string;
	from: number;
	to: number;
}

class TableWidget extends WidgetType {
	constructor(private readonly rows: TableCell[][]) {
		super();
	}

	eq(widget: TableWidget) {
		return JSON.stringify(widget.rows) === JSON.stringify(this.rows);
	}

	toDOM(view: EditorView) {
		const table = document.createElement('table');
		table.className = 'cm-md-table';
		this.rows.forEach((row, rowIndex) => {
			if (
				rowIndex === 1 &&
				row.every((cell) => /^:?-+:?$/.test(cell.text.trim()))
			) {
				return;
			}
			const container =
				rowIndex === 0
					? document.createElement('thead')
					: document.createElement('tbody');
			const tr = document.createElement('tr');
			for (const cell of row) {
				const element = document.createElement(rowIndex === 0 ? 'th' : 'td');
				element.textContent = cell.text.trim();
				element.tabIndex = 0;
				const reveal = () => {
					view.dispatch({
						selection: { anchor: cell.from, head: cell.to },
					});
					view.focus();
				};
				element.addEventListener('click', reveal);
				element.addEventListener('keydown', (event) => {
					if (event.key === 'Enter' || event.key === ' ') {
						event.preventDefault();
						reveal();
					}
				});
				tr.appendChild(element);
			}
			container.appendChild(tr);
			table.appendChild(container);
		});
		return table;
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

interface PreviewSpecs {
	resolveImage?: LiveMarkdownOptions['resolveImage'];
	resolveLink?: LiveMarkdownOptions['resolveLink'];
	openPdf?: LiveMarkdownOptions['openPdf'];
	mountPdfEmbed?: LiveMarkdownOptions['mountPdfEmbed'];
}

function previewWidgetFactories(specs: PreviewSpecs) {
	return {
		image: (alt: string, src: string) =>
			new ImageWidget(alt, src, specs.resolveImage),
		math: (source: string, displayMode: boolean) =>
			new MathWidget(source, displayMode),
		table: (rows: TableCell[][]) => new TableWidget(rows),
		link: (
			target: string,
			label: string,
			embed: boolean,
			from: number,
			to: number,
		) =>
			isPdfTarget(target)
				? new PdfWidget(target, label, embed, from, to, specs)
				: new LinkWidget(target, label, embed, from, to, specs.resolveLink),
	};
}

function fullDocument(state: EditorState): readonly PreviewViewportRange[] {
	return [{ from: 0, to: state.doc.length }];
}

const setViewportEffect = StateEffect.define<readonly PreviewViewportRange[]>();

const viewportSyncPlugin = ViewPlugin.fromClass(
	class {
		constructor(readonly view: EditorView) {}
		update(update: { viewportChanged: boolean }) {
			if (!update.viewportChanged) return;
			// Transactions may not be dispatched while an update is running.
			// A microtask keeps the effect dispatch outside the update cycle
			// while still landing before the frame is painted.
			Promise.resolve().then(() => {
				this.view.dispatch({
					effects: setViewportEffect.of(this.view.visibleRanges),
				});
			});
		}
	},
);

/**
 * Preview decorations live in a state field, not a view plugin: CodeMirror
 * rejects block widgets and multi-line replaces that were provided through
 * a plugin (the facet function path runs after layout), while
 * `EditorView.decorations.from(field)` is the supported layout-capable path.
 */
export function createPdfPreviewExtensions(specs: PreviewSpecs): Extension[] {
	return [createPreviewField(specs, true), viewportSyncPlugin];
}

function createPreviewField(specs: PreviewSpecs, pdfOnly = false) {
	const factories = previewWidgetFactories(specs);
	const decorate = (
		state: EditorState,
		viewport: readonly PreviewViewportRange[],
	) => {
		const decorations = buildDecorations(
			state,
			viewport,
			Decoration,
			factories,
		);
		return pdfOnly
			? decorations.update({
					filter: (_from, _to, decoration) =>
						decoration.spec.widget instanceof PdfWidget,
				})
			: decorations;
	};
	return StateField.define<{
		decorations: DecorationSet;
		viewport: readonly PreviewViewportRange[];
	}>({
		create: (state) => ({
			viewport: fullDocument(state),
			decorations: decorate(state, fullDocument(state)),
		}),
		update(value, transaction) {
			const viewportEffect = transaction.effects.reduce<
				readonly PreviewViewportRange[] | null
			>(
				(found, effect) =>
					effect.is(setViewportEffect) ? effect.value : found,
				null,
			);
			if (
				!transaction.docChanged &&
				transaction.selection === undefined &&
				viewportEffect === null
			) {
				return value;
			}
			const viewport =
				viewportEffect ??
				(transaction.docChanged
					? value.viewport.map((range) => ({
							from: transaction.changes.mapPos(range.from, -1),
							to: transaction.changes.mapPos(range.to, 1),
						}))
					: value.viewport);
			return {
				viewport,
				decorations: decorate(transaction.state, viewport),
			};
		},
		provide: (field) =>
			EditorView.decorations.from(field, (value) => value.decorations),
	});
}

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
	const reportSelection = (view: EditorView) => {
		const range = view.state.selection.main;
		const coordinates = range.empty ? null : view.coordsAtPos(range.head);
		const line = view.state.doc.lineAt(range.head);
		options.onSelectionChange?.({
			from: range.from,
			to: range.to,
			anchor: coordinates
				? {
						left: coordinates.left,
						top: coordinates.top,
						bottom: coordinates.bottom,
					}
				: null,
			hasFocus: view.hasFocus,
			composing: view.composing,
			blockStyle: markdownBlockStyle(line.text),
		});
	};
	const extensions: Extension[] = [
		markdown({ base: markdownLanguage, extensions: [GFM] }),
		EditorView.lineWrapping,
		createPreviewField({
			resolveImage: options.resolveImage,
			resolveLink: options.resolveLink,
			openPdf: options.openPdf,
			mountPdfEmbed: options.mountPdfEmbed,
		}),
		viewportSyncPlugin,
		checkboxClickPlugin,
		keymap.of([indentWithTab]),
		EditorView.updateListener.of((update) => {
			const canonical = update.transactions.some(
				(transaction) => transaction.annotation(canonicalUpdate) === true,
			);
			if (update.docChanged && !canonical) options.onChange?.(update.view);
			if (
				update.docChanged ||
				update.selectionSet ||
				update.focusChanged ||
				update.viewportChanged
			) {
				reportSelection(update.view);
			}
		}),
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
			'.cm-md-link-preview': {
				color: 'var(--primary)',
				textDecoration: 'underline',
				textUnderlineOffset: '0.2em',
				cursor: 'pointer',
			},
			'.cm-md-link-preview[data-state="unresolved"]': {
				color: 'var(--muted-foreground)',
			},
			'.cm-md-embed-preview': {
				display: 'flex',
				flexDirection: 'column',
				gap: '0.25rem',
				margin: '0.75rem 0',
				padding: '0.75rem 1rem',
				borderLeft: '2px solid var(--border)',
				backgroundColor: 'var(--muted)',
				cursor: 'pointer',
			},
			'.cm-md-embed-preview p': {
				margin: '0',
				whiteSpace: 'pre-wrap',
				color: 'var(--muted-foreground)',
			},
		}),
		EditorState.readOnly.of(options.readOnly === true),
		EditorView.editable.of(options.readOnly !== true),
	];
	let undoManager: Y.UndoManager | null = null;
	if (options.collaborative !== false) {
		undoManager = new Y.UndoManager(options.ytext);
		// WebKit does not reliably fire `beforeinput` with `historyUndo`, so
		// the shared Yjs undo stack also needs an explicit keymap.
		extensions.push(
			yCollab(options.ytext, null, { undoManager }),
			keymap.of(yUndoManagerKeymap),
		);
	} else {
		extensions.push(history(), keymap.of([...historyKeymap]));
	}
	const view = new EditorView({
		state: EditorState.create({ doc: doc.toString(), extensions }),
		parent,
	});
	const format = (name: MarkdownFormat) => {
		if (name === 'text') return formattingCommands.heading(0)(view);
		if (name === 'heading1') return formattingCommands.heading(1)(view);
		if (name === 'heading2') return formattingCommands.heading(2)(view);
		if (name === 'heading3') return formattingCommands.heading(3)(view);
		formattingCommands[name](view);
	};
	return {
		view,
		doc: () => view.state.doc.toString(),
		setText: (text: string) => {
			// Apply canonical reloads and reconciled drafts through CodeMirror so
			// selections are mapped with the same transaction that changes the
			// visible document. The Yjs binding mirrors this transaction into the
			// Y.Text; mutating Y.Text directly can leave a focused WebKit editor
			// displaying its previous state until another local transaction occurs.
			applyCanonicalText(view, text, undoManager);
		},
		format,
		undo: () => undoManager?.undo(),
		redo: () => undoManager?.redo(),
		focus: () => view.focus(),
		destroy: () => view.destroy(),
	};
}
