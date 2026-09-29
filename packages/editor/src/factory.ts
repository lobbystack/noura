import { PdfWidget } from './pdf-widget';
import { isPdfTarget } from './pdf-target';
import {
	defaultKeymap,
	history,
	historyKeymap,
	indentWithTab,
	redo,
	undo,
} from '@codemirror/commands';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { languages } from '@codemirror/language-data';
import { syntaxHighlighting, syntaxTree } from '@codemirror/language';
import {
	autocompletion,
	type CompletionSource,
} from '@codemirror/autocomplete';
import { openSearchPanel, search, searchKeymap } from '@codemirror/search';
import { classHighlighter } from '@lezer/highlight';
import { GFM } from '@lezer/markdown';
import {
	Decoration,
	EditorView,
	ViewPlugin,
	keymap,
	type DecorationSet,
} from '@codemirror/view';
import {
	Annotation,
	ChangeSet,
	Compartment,
	EditorSelection,
	EditorState,
	Prec,
	StateEffect,
	StateField,
	Transaction,
	type Extension,
} from '@codemirror/state';
import { buildDecorations, type PreviewViewportRange } from './preview';
import {
	activeFormats,
	formattingCommands,
	formattingKeymap,
	toggleCheckboxes,
} from './commands';
import { markdownSyntax } from './syntax';
import { changesBetween, rebaseChanges } from './text-sync';
import {
	CheckboxWidget,
	ImageWidget,
	LinkWidget,
	MathWidget,
	TableWidget,
} from './widgets';
import type {
	LinkKind,
	LinkSuggestion,
	LiveMarkdownEditor,
	LiveMarkdownOptions,
	MarkdownBlockStyle,
	MarkdownFormat,
} from './types';

/** Marks transactions that load canonical text rather than user edits. */
const canonicalUpdate = Annotation.define<boolean>();

export function markdownBlockStyle(line: string): MarkdownBlockStyle {
	if (/^#\s/.test(line)) return 'heading1';
	if (/^##\s/.test(line)) return 'heading2';
	if (/^###\s/.test(line)) return 'heading3';
	return 'text';
}

/**
 * Text semantics shared by every editor: `\n` is the only line separator, so
 * a lone `\r` stays an ordinary character and positions equal string
 * indices. Pasted Windows line endings become `\n`.
 */
export const textSemantics: Extension = [
	EditorState.lineSeparator.of('\n'),
	EditorView.clipboardInputFilter.of((text) => text.replace(/\r\n?/g, '\n')),
];

/** Markdown with GFM, frontmatter, display math, and lazy code languages. */
export function markdownSupport(): Extension {
	return markdown({
		base: markdownLanguage,
		extensions: [GFM, ...markdownSyntax],
		codeLanguages: languages,
	});
}

interface PreviewSpecs {
	resolveImage?: LiveMarkdownOptions['resolveImage'] | undefined;
	resolveLink?: LiveMarkdownOptions['resolveLink'] | undefined;
	openLink?: LiveMarkdownOptions['openLink'] | undefined;
	openPdf?: LiveMarkdownOptions['openPdf'] | undefined;
	mountPdfEmbed?: LiveMarkdownOptions['mountPdfEmbed'] | undefined;
}

function previewWidgetFactories(specs: PreviewSpecs) {
	return {
		image: (
			alt: string,
			src: string,
			options?: { width?: number | undefined; title?: string | undefined },
		) => new ImageWidget(alt, src, options ?? {}, specs.resolveImage),
		math: (source: string, displayMode: boolean) =>
			new MathWidget(source, displayMode),
		table: (
			source: string,
			rows: Array<Array<{ text: string; from: number; to: number }>>,
		) => new TableWidget(source, rows),
		link: (target: string, label: string, embed: boolean) =>
			isPdfTarget(target)
				? new PdfWidget(target, label, embed, specs)
				: new LinkWidget(target, label, embed, specs),
		checkbox: (checked: boolean) => new CheckboxWidget(checked),
	};
}

/**
 * Decorations cover the visible ranges only. Until the view reports them,
 * the start of the document stands in, so a large file opens without
 * decorating all of it.
 */
const INITIAL_VIEWPORT = 16_384;

const setViewportEffect = StateEffect.define<readonly PreviewViewportRange[]>();

function sameRanges(
	left: readonly PreviewViewportRange[],
	right: readonly PreviewViewportRange[],
) {
	return (
		left.length === right.length &&
		left.every(
			(range, index) =>
				range.from === right[index]?.from && range.to === right[index]?.to,
		)
	);
}

const viewportSyncPlugin = ViewPlugin.fromClass(
	class {
		private destroyed = false;
		constructor(readonly view: EditorView) {
			this.schedule();
		}
		update(update: { viewportChanged: boolean }) {
			if (update.viewportChanged) this.schedule();
		}
		destroy() {
			this.destroyed = true;
		}
		private schedule() {
			// Transactions may not be dispatched while an update is running.
			// A microtask keeps the effect dispatch outside the update cycle
			// while still landing before the frame is painted.
			void Promise.resolve().then(() => {
				if (this.destroyed) return;
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
		create: (state) => {
			const viewport = [
				{ from: 0, to: Math.min(state.doc.length, INITIAL_VIEWPORT) },
			];
			return { viewport, decorations: decorate(state, viewport) };
		},
		update(value, transaction) {
			let viewportEffect: readonly PreviewViewportRange[] | null = null;
			for (const effect of transaction.effects) {
				if (effect.is(setViewportEffect)) viewportEffect = effect.value;
			}
			if (viewportEffect && sameRanges(viewportEffect, value.viewport)) {
				viewportEffect = null;
			}
			// The background parser extends the tree after the first pass;
			// decorations must follow, or long files stay unstyled past the
			// first few kilobytes until the next edit.
			const treeChanged =
				syntaxTree(transaction.state) !== syntaxTree(transaction.startState);
			if (
				!transaction.docChanged &&
				transaction.selection === undefined &&
				viewportEffect === null &&
				!treeChanged
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

/** Checkbox toggles and link following, handled before CodeMirror moves the caret. */
function previewClicks(openLink: LiveMarkdownOptions['openLink']) {
	return EditorView.domEventHandlers({
		mousedown(event, view) {
			const target = event.target as HTMLElement | null;
			if (!target || event.button !== 0) return false;
			const box = target.closest<HTMLElement>('.cm-md-checkbox');
			if (box) {
				if (view.state.readOnly) return false;
				event.preventDefault();
				const line = view.state.doc.lineAt(view.posAtDOM(box));
				toggleCheckboxes(view, [line.number]);
				return true;
			}
			const link = target.closest<HTMLElement>('[data-href]');
			const href = link?.dataset.href;
			if (!link || !href || !openLink) return false;
			const newTab = event.metaKey || event.ctrlKey;
			// Rendered links follow on a plain click, like Obsidian's live
			// preview; visible link source needs Cmd/Ctrl so clicking still
			// places the caret for editing.
			if (!newTab && !link.classList.contains('cm-md-link-rendered')) {
				return false;
			}
			event.preventDefault();
			openLink(href, {
				kind: (link.dataset.linkKind as LinkKind | undefined) ?? 'markdown',
				newTab,
			});
			return true;
		},
	});
}

/** `[[` completion from the workspace's files, like Obsidian. */
function linkCompletion(
	suggestions: () => readonly LinkSuggestion[],
): CompletionSource {
	return (context) => {
		const match = context.matchBefore(/\[\[[^\]\n|#]*$/);
		if (!match) return null;
		const from = match.from + 2;
		const closed =
			context.state.sliceDoc(context.pos, context.pos + 2) === ']]';
		return {
			from,
			options: suggestions().map((suggestion) => ({
				label: suggestion.label,
				...(suggestion.detail ? { detail: suggestion.detail } : {}),
				type: 'file',
				apply: closed ? suggestion.target : `${suggestion.target}]]`,
			})),
			validFor: /^[^\]\n|#]*$/,
		};
	};
}

const spelling = new Compartment();

/** CodeMirror turns spellcheck off by default; this restores the platform's. */
export function spellcheckAttributes(enabled: boolean): Extension {
	return EditorView.contentAttributes.of({
		spellcheck: enabled ? 'true' : 'false',
		autocorrect: enabled ? 'on' : 'off',
	});
}

/** A reconfigurable spellcheck setting for any editor. */
export function spellcheckExtension(enabled: boolean): Extension {
	return spelling.of(spellcheckAttributes(enabled));
}

/** Follow the spellcheck preference in an editor made with {@link spellcheckExtension}. */
export function setSpellcheck(view: EditorView, enabled: boolean) {
	view.dispatch({
		effects: spelling.reconfigure(spellcheckAttributes(enabled)),
	});
}

const editorTheme = EditorView.theme({
	'&': {
		backgroundColor: 'transparent',
		height: '100%',
		fontSize: 'var(--content-text-size, 1rem)',
	},
	'.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.75' },
	'.cm-content': { caretColor: 'var(--foreground)' },
	'.cm-md-heading': { fontWeight: '600' },
	'.cm-md-h1': {
		fontSize: 'var(--content-heading-1-size, 2rem)',
		lineHeight: '1.2',
		padding: '0.6rem 0',
	},
	'.cm-md-h2': {
		fontSize: 'var(--content-heading-2-size, 1.5rem)',
		lineHeight: '1.25',
		padding: '0.5rem 0',
	},
	'.cm-md-h3': {
		fontSize: 'var(--content-heading-3-size, 1.25rem)',
		lineHeight: '1.3',
		padding: '0.4rem 0',
	},
	'.cm-md-h4': {
		fontSize: 'var(--content-heading-4-size, 1.125rem)',
		lineHeight: '1.4',
	},
	'.cm-md-strong': { fontWeight: '700' },
	'.cm-md-emphasis': { fontStyle: 'italic' },
	'.cm-md-strike': { textDecoration: 'line-through' },
	'.cm-md-url': { color: 'var(--muted-foreground)' },
	'.cm-md-list-mark': { color: 'var(--muted-foreground)' },
	'.cm-md-link, .cm-md-link-preview': {
		color: 'var(--primary)',
		textDecoration: 'underline',
		textUnderlineOffset: '0.2em',
	},
	'.cm-md-link-rendered, .cm-md-link-preview': { cursor: 'pointer' },
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
	'.cm-md-frontmatter': {
		fontFamily: 'var(--font-mono, monospace)',
		fontSize: 'var(--content-text-size, 1rem)',
		color: 'var(--muted-foreground)',
		backgroundColor: 'var(--muted)',
		paddingInline: '0.75rem',
	},
	'.cm-md-codeblock': {
		fontFamily: 'var(--font-mono, monospace)',
		fontSize: 'var(--content-text-size, 1rem)',
		backgroundColor: 'var(--muted)',
		paddingInline: '0.75rem',
	},
	'.cm-md-codeblock-begin': {
		borderTopLeftRadius: '0.375rem',
		borderTopRightRadius: '0.375rem',
	},
	'.cm-md-codeblock-end': {
		borderBottomLeftRadius: '0.375rem',
		borderBottomRightRadius: '0.375rem',
	},
	'.cm-md-code-fence': { color: 'var(--muted-foreground)' },
	'.cm-md-codeblock .tok-keyword, .cm-md-codeblock .tok-operatorKeyword': {
		color: 'var(--chart-4, #8250df)',
	},
	'.cm-md-codeblock .tok-string, .cm-md-codeblock .tok-string2': {
		color: 'var(--chart-2, #0a7f4f)',
	},
	'.cm-md-codeblock .tok-number, .cm-md-codeblock .tok-bool, .cm-md-codeblock .tok-atom':
		{ color: 'var(--chart-1, #b35900)' },
	'.cm-md-codeblock .tok-comment': {
		color: 'var(--muted-foreground)',
		fontStyle: 'italic',
	},
	'.cm-md-codeblock .tok-typeName, .cm-md-codeblock .tok-className': {
		color: 'var(--chart-3, #0550ae)',
	},
	'.cm-md-codeblock .tok-definition, .cm-md-codeblock .tok-propertyName': {
		color: 'var(--chart-5, #953800)',
	},
	'.cm-md-math-source': { fontFamily: 'var(--font-mono, monospace)' },
	'.cm-md-callout': {
		backgroundColor: 'var(--muted)',
		borderLeft: '2px solid var(--primary)',
		paddingLeft: '0.75rem',
	},
	'.cm-md-callout-title': { fontWeight: '600' },
	'.cm-md-callout-type': { color: 'var(--muted-foreground)' },
	'.cm-md-task-line .cm-md-checkbox': { verticalAlign: 'middle' },
	'.cm-md-checkbox[data-checked="true"]': {
		backgroundImage:
			"url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M3.5 8.5l3 3 6-7'/%3E%3C/svg%3E\")",
		backgroundSize: '100% 100%',
	},
	'.cm-md-hr-line': {
		backgroundImage: 'linear-gradient(var(--border), var(--border))',
		backgroundSize: '100% 1px',
		backgroundPosition: 'center',
		backgroundRepeat: 'no-repeat',
	},
	'.cm-panels': {
		backgroundColor: 'var(--background)',
		color: 'var(--foreground)',
	},
	'.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--border)' },
	'.cm-search': {
		display: 'flex',
		flexWrap: 'wrap',
		alignItems: 'center',
		gap: '0.25rem 0.5rem',
		padding: '0.5rem 1rem',
		fontSize: 'var(--text-sm, 0.875rem)',
	},
	'.cm-textfield': {
		border: '1px solid var(--input)',
		borderRadius: '0.375rem',
		backgroundColor: 'transparent',
		padding: '0.125rem 0.5rem',
	},
	'.cm-button': {
		backgroundImage: 'none',
		backgroundColor: 'transparent',
		border: '1px solid var(--border)',
		borderRadius: '0.375rem',
		padding: '0.125rem 0.5rem',
	},
	'.cm-searchMatch': { backgroundColor: 'var(--accent)' },
	'.cm-searchMatch-selected': {
		backgroundColor: 'var(--primary)',
		color: 'var(--primary-foreground)',
	},
	'.cm-tooltip': {
		backgroundColor: 'var(--popover)',
		color: 'var(--popover-foreground)',
		border: '1px solid var(--border)',
		borderRadius: '0.5rem',
		overflow: 'hidden',
	},
	'.cm-tooltip-autocomplete > ul > li': { padding: '0.25rem 0.5rem' },
	'.cm-tooltip-autocomplete > ul > li[aria-selected]': {
		backgroundColor: 'var(--accent)',
		color: 'var(--accent-foreground)',
	},
	'.cm-completionDetail': {
		marginLeft: '0.75rem',
		fontStyle: 'normal',
		color: 'var(--muted-foreground)',
	},
});

function clampPosition(position: number, length: number) {
	return Math.max(0, Math.min(length, position));
}

export function createLiveMarkdownEditor(
	parent: HTMLElement,
	options: LiveMarkdownOptions,
): LiveMarkdownEditor {
	const readOnly = options.readOnly === true;
	const reportSelection = (view: EditorView) => {
		if (!options.onSelectionChange) return;
		const range = view.state.selection.main;
		const coordinates = range.empty ? null : view.coordsAtPos(range.head);
		const line = view.state.doc.lineAt(range.head);
		options.onSelectionChange({
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
			formats: activeFormats(view.state),
		});
	};
	const extensions: Extension[] = [
		textSemantics,
		markdownSupport(),
		syntaxHighlighting(classHighlighter),
		EditorView.lineWrapping,
		createPreviewField({
			resolveImage: options.resolveImage,
			resolveLink: options.resolveLink,
			openLink: options.openLink,
			openPdf: options.openPdf,
			mountPdfEmbed: options.mountPdfEmbed,
		}),
		viewportSyncPlugin,
		previewClicks(options.openLink),
		Prec.high(keymap.of(formattingKeymap)),
		history(),
		search({ top: true }),
		keymap.of([
			...defaultKeymap,
			...historyKeymap,
			...searchKeymap,
			indentWithTab,
		]),
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
		editorTheme,
		EditorState.readOnly.of(readOnly),
		EditorView.editable.of(!readOnly),
		spellcheckExtension(options.spellcheck === true),
		EditorView.contentAttributes.of({
			'aria-label': options.label ?? 'Markdown editor',
		}),
	];
	if (options.linkSuggestions && !readOnly) {
		extensions.push(
			autocompletion({
				override: [linkCompletion(options.linkSuggestions)],
				icons: false,
			}),
		);
	}
	const length = options.text.length;
	const restore = options.restore;
	const view = new EditorView({
		state: EditorState.create({
			doc: options.text,
			extensions,
			...(restore
				? {
						selection: EditorSelection.single(
							clampPosition(restore.anchor, length),
							clampPosition(restore.head, length),
						),
					}
				: {}),
		}),
		parent,
		...(restore
			? {
					scrollTo: EditorView.scrollIntoView(
						clampPosition(restore.top, length),
						{ y: 'start' },
					),
				}
			: {}),
	});
	const applyCanonical = (changes: ChangeSet) => {
		if (changes.empty) return;
		// Canonical text is a new baseline, not an edit: it stays out of the
		// undo history (undo cannot bring back stale text) while earlier edits
		// remain undoable, mapped through the change.
		view.dispatch({
			changes,
			annotations: [
				canonicalUpdate.of(true),
				Transaction.addToHistory.of(false),
			],
		});
	};
	const format = (name: MarkdownFormat) => {
		if (readOnly) return;
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
			const current = view.state.doc.toString();
			if (current !== text) applyCanonical(changesBetween(current, text));
		},
		rebase: (base: string, target: string) => {
			if (base === target) return;
			applyCanonical(rebaseChanges(base, view.state.doc.toString(), target));
		},
		format,
		undo: () => undo(view),
		redo: () => redo(view),
		focus: () => view.focus(),
		destroy: () => view.destroy(),
		setSpellcheck: (enabled: boolean) => setSpellcheck(view, enabled),
		memory: () => {
			const range = view.state.selection.main;
			return {
				anchor: range.anchor,
				head: range.head,
				top: view.lineBlockAtHeight(view.scrollDOM.scrollTop).from,
			};
		},
		find: () => {
			openSearchPanel(view);
		},
	};
}
