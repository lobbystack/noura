import { isPdfTarget } from './pdf-target';
import { syntaxTree } from '@codemirror/language';
import type { Decoration, DecorationSet, WidgetType } from '@codemirror/view';
import { RangeSet, type EditorState } from '@codemirror/state';
import type { SyntaxNode } from '@lezer/common';

/** One visible document span. Mirrors `EditorView.visibleRanges`. */
export interface PreviewViewportRange {
	from: number;
	to: number;
}

/** One table cell, with offsets relative to the start of the table. */
export interface TableCell {
	text: string;
	from: number;
	to: number;
}

export interface ImageOptions {
	/** Obsidian's `![[image.png|300]]` width, in CSS pixels. */
	width?: number | undefined;
	title?: string | undefined;
}

export interface PreviewWidgetFactories {
	image: (
		alt: string,
		src: string,
		options?: ImageOptions,
	) => WidgetType | null;
	math: (source: string, displayMode: boolean) => WidgetType;
	table: (source: string, rows: TableCell[][]) => WidgetType;
	link: (target: string, label: string, embed: boolean) => WidgetType | null;
	checkbox?: (checked: boolean) => WidgetType;
}

/** File extensions `![[...]]` renders as an image. */
const IMAGE_TARGET = /\.(?:png|jpe?g|gif|webp|svg|bmp|avif)$/i;

function isImageTarget(target: string): boolean {
	return IMAGE_TARGET.test(target.split('#')[0] ?? '');
}

const TASK_LINE = /^(\s*)([-*+]|\d+[.)])(\s+)\[([ xX])\](?=\s|$)/;

/** Parse a task list line: bullet or ordered, with or without text. */
export function taskMarker(text: string) {
	const match = TASK_LINE.exec(text);
	if (!match) return null;
	const markerFrom = (match[1] ?? '').length;
	const boxFrom =
		markerFrom + (match[2] ?? '').length + (match[3] ?? '').length;
	return {
		markerFrom,
		boxFrom,
		boxTo: boxFrom + 3,
		bullet: /^[-*+]$/.test(match[2] ?? ''),
		checked: match[4] !== ' ',
	};
}

function headingLineClass(node: SyntaxNode): string | null {
	if (node.name === 'ATXHeading1') return 'cm-md-heading cm-md-h1';
	if (node.name === 'ATXHeading2') return 'cm-md-heading cm-md-h2';
	if (node.name === 'ATXHeading3') return 'cm-md-heading cm-md-h3';
	if (node.name.startsWith('ATXHeading')) return 'cm-md-heading cm-md-h4';
	return null;
}

const inlineMathPattern = /(?<!\$)\$(?![\s$])([^$\n]*?\S)\$(?![$\d])/g;

export function inlineMathMatches(text: string) {
	return Array.from(text.matchAll(inlineMathPattern), (match) => ({
		index: match.index,
		source: match[1] ?? '',
		text: match[0],
	}));
}

interface CollectedDecoration {
	from: number;
	to: number;
	deco: Decoration;
	/** True for replace decorations, which hide the text they cover. */
	replaces: boolean;
}

/** Inline nodes whose markers hide while the selection is elsewhere. */
const INLINE_MARKS: Record<string, { mark: string; className: string }> = {
	Emphasis: { mark: 'EmphasisMark', className: 'cm-md-emphasis' },
	StrongEmphasis: { mark: 'EmphasisMark', className: 'cm-md-strong' },
	Strikethrough: { mark: 'StrikethroughMark', className: 'cm-md-strike' },
	InlineCode: { mark: 'CodeMark', className: 'cm-md-code' },
};

/** Block nodes whose lines keep their literal text. */
const LITERAL_BLOCKS = new Set([
	'FencedCode',
	'CodeBlock',
	'Frontmatter',
	'MathBlock',
	'HTMLBlock',
	'CommentBlock',
]);

/**
 * Syntax-tree marks and line classes for inline styling, plus replace
 * decorations for checkboxes, images, tables, math, and Obsidian syntax,
 * limited to the visible ranges. Syntax the selection touches stays raw so
 * editing never fights the editor.
 *
 * Block replace decorations (tables, display math, standalone embeds) are
 * only legal in a decoration set provided through
 * `EditorView.decorations.from(stateField)`; this function stays a pure
 * function of `EditorState` so the owning field can provide exactly that.
 */
export function buildDecorations(
	state: EditorState,
	viewport: readonly PreviewViewportRange[],
	deco: typeof Decoration,
	widgets: PreviewWidgetFactories,
): DecorationSet {
	const pushed: CollectedDecoration[] = [];
	const seenLines = new Set<string>();
	const addMark = (
		from: number,
		to: number,
		cls: string,
		attributes?: Record<string, string>,
	) => {
		if (to > from) {
			pushed.push({
				from,
				to,
				deco: deco.mark(
					attributes ? { class: cls, attributes } : { class: cls },
				),
				replaces: false,
			});
		}
	};
	const addLine = (lineFrom: number, cls: string) => {
		// Visible ranges can overlap one block; keep one class per line.
		const key = `${lineFrom}:${cls}`;
		if (seenLines.has(key)) return;
		seenLines.add(key);
		pushed.push({
			from: lineFrom,
			to: lineFrom,
			deco: deco.line({ class: cls }),
			replaces: false,
		});
	};
	const addReplace = (
		from: number,
		to: number,
		spec: { widget?: WidgetType; block?: boolean } | Record<string, never>,
	) => {
		if (to <= from) return;
		pushed.push({ from, to, deco: deco.replace(spec), replaces: true });
	};
	// Read-only previews render everything; an editor reveals the syntax the
	// selection touches.
	const revealed = (from: number, to: number) =>
		!state.readOnly &&
		state.selection.ranges.some(
			(range) => range.from <= to && range.to >= from,
		);
	const eachLine = (
		from: number,
		to: number,
		apply: (lineFrom: number) => void,
	) => {
		const last = state.doc.lineAt(Math.max(from, to - 1)).number;
		for (
			let number = state.doc.lineAt(from).number;
			number <= last;
			number += 1
		) {
			apply(state.doc.line(number).from);
		}
	};
	const context: DecorationContext = {
		state,
		widgets,
		addMark,
		addLine,
		addReplace,
		revealed,
		eachLine,
	};
	const tree = syntaxTree(state);
	for (const visible of viewport) {
		tree.iterate({
			from: visible.from,
			to: visible.to,
			enter: (ref): boolean | void => decorateNode(context, ref.node),
		});
		decorateObsidianSyntax(context, visible.from, visible.to);
	}
	return finishDecorationSet(pushed);
}

interface DecorationContext {
	state: EditorState;
	widgets: PreviewWidgetFactories;
	addMark: (
		from: number,
		to: number,
		cls: string,
		attributes?: Record<string, string>,
	) => void;
	addLine: (lineFrom: number, cls: string) => void;
	addReplace: (
		from: number,
		to: number,
		spec: { widget?: WidgetType; block?: boolean } | Record<string, never>,
	) => void;
	revealed: (from: number, to: number) => boolean;
	eachLine: (
		from: number,
		to: number,
		apply: (lineFrom: number) => void,
	) => void;
}

function decorateNode(cx: DecorationContext, node: SyntaxNode): boolean | void {
	const { state, addMark, addLine, addReplace, revealed, eachLine } = cx;
	const heading = headingLineClass(node);
	if (heading) {
		const line = state.doc.lineAt(node.from);
		addLine(line.from, heading);
		// Hide the # marker when the cursor is elsewhere.
		const match = line.text.match(/^(#{1,6})\s+/);
		if (match && !revealed(node.from, line.to)) {
			addReplace(line.from, line.from + match[0].length, {});
		}
		return;
	}
	const inline = INLINE_MARKS[node.name];
	if (inline) {
		addMark(node.from, node.to, inline.className);
		if (!revealed(node.from, node.to)) {
			for (let child = node.firstChild; child; child = child.nextSibling) {
				if (child.name === inline.mark) addReplace(child.from, child.to, {});
			}
		}
		// Code spans stay literal; other nodes may nest (bold inside italic).
		return node.name === 'InlineCode' ? false : undefined;
	}
	switch (node.name) {
		case 'Frontmatter':
			eachLine(node.from, node.to, (from) =>
				addLine(from, 'cm-md-frontmatter'),
			);
			return false;
		case 'MathBlock':
			return decorateMathBlock(cx, node);
		case 'FencedCode':
		case 'CodeBlock': {
			const first = state.doc.lineAt(node.from).from;
			const last = state.doc.lineAt(Math.max(node.from, node.to - 1)).from;
			eachLine(node.from, node.to, (from) => {
				addLine(from, 'cm-md-codeblock');
				if (node.name === 'FencedCode' && from === first) {
					addLine(from, 'cm-md-codeblock-begin');
				}
				if (node.name === 'FencedCode' && from === last) {
					addLine(from, 'cm-md-codeblock-end');
				}
			});
			for (let child = node.firstChild; child; child = child.nextSibling) {
				if (child.name === 'CodeMark' || child.name === 'CodeInfo') {
					addMark(child.from, child.to, 'cm-md-code-fence');
				}
			}
			return false;
		}
		case 'Blockquote':
			decorateBlockquote(cx, node);
			return;
		case 'Link':
		case 'Image':
			return decorateLink(cx, node);
		case 'Autolink':
		case 'URL': {
			// URLs inside links and images belong to their parent.
			const parent = node.parent?.name;
			if (node.name === 'URL' && (parent === 'Link' || parent === 'Image')) {
				return;
			}
			const url = state.sliceDoc(node.from, node.to).replace(/^<|>$/g, '');
			addMark(node.from, node.to, 'cm-md-url', {
				'data-href': url,
				'data-link-kind': 'url',
			});
			return false;
		}
		case 'SetextHeading1':
		case 'SetextHeading2': {
			const underline = node.lastChild;
			const textEnd = underline ? underline.from : node.to;
			cx.eachLine(node.from, Math.max(node.from + 1, textEnd - 1), (from) =>
				addLine(
					from,
					node.name === 'SetextHeading1'
						? 'cm-md-heading cm-md-h1'
						: 'cm-md-heading cm-md-h2',
				),
			);
			if (underline?.name === 'HeaderMark') {
				addMark(underline.from, underline.to, 'cm-md-list-mark');
			}
			return;
		}
		case 'ListMark':
		case 'QuoteMark':
			addMark(node.from, node.to, 'cm-md-list-mark');
			return;
		case 'HorizontalRule': {
			const line = state.doc.lineAt(node.from);
			addLine(line.from, 'cm-md-hr-line');
			if (!revealed(line.from, line.to)) addReplace(node.from, node.to, {});
			return;
		}
		case 'Table':
			return decorateTable(cx, node);
		default:
			// Task checkboxes come from the line scan, which also covers empty
			// tasks the GFM parser does not recognize.
			return;
	}
}

function decorateMathBlock(cx: DecorationContext, node: SyntaxNode) {
	const { state, addLine, addReplace, revealed, eachLine, widgets } = cx;
	const startLine = state.doc.lineAt(node.from);
	const endLine = state.doc.lineAt(Math.max(node.from, node.to - 1));
	if (!revealed(startLine.from, endLine.to)) {
		const source = state
			.sliceDoc(node.from, node.to)
			.trim()
			.replace(/^\$\$/, '')
			.replace(/\$\$$/, '')
			.trim();
		addReplace(startLine.from, endLine.to, {
			widget: widgets.math(source, true),
			block: true,
		});
	} else {
		eachLine(node.from, node.to, (from) => addLine(from, 'cm-md-math-source'));
	}
	return false;
}

const CALLOUT = /^(\s*>\s*)(\[![^\]]+\][+-]?)(\s*)/;

function decorateBlockquote(cx: DecorationContext, node: SyntaxNode) {
	const { state, addLine, addMark, addReplace, revealed, eachLine } = cx;
	const firstLine = state.doc.lineAt(node.from);
	const callout = CALLOUT.exec(firstLine.text);
	const quoteClass = callout ? 'cm-md-callout' : 'cm-md-quote-line';
	eachLine(node.from, node.to, (from) => addLine(from, quoteClass));
	if (!callout) return;
	addLine(firstLine.from, 'cm-md-callout-title');
	const marker = callout[2] ?? '';
	const typeFrom = firstLine.from + (callout[1] ?? '').length;
	const typeTo = typeFrom + marker.length;
	const lastLine = state.doc.lineAt(Math.max(node.from, node.to - 1));
	// Like Obsidian, the source shows while the caret is anywhere in the
	// callout; otherwise the `>` markers hide and `[!note]` reads "note".
	if (revealed(firstLine.from, lastLine.to)) {
		addMark(typeFrom, typeTo, 'cm-md-callout-type');
		return;
	}
	eachLine(node.from, node.to, (from) => {
		const quote = /^(\s*)>[ \t]?/.exec(state.doc.lineAt(from).text);
		if (quote)
			addReplace(from + (quote[1] ?? '').length, from + quote[0].length, {});
	});
	const close = marker.indexOf(']');
	addReplace(typeFrom, typeFrom + 2, {});
	addMark(typeFrom + 2, typeFrom + close, 'cm-md-callout-label');
	addReplace(typeFrom + close, typeTo, {});
}

function linkParts(state: EditorState, node: SyntaxNode) {
	const marks: SyntaxNode[] = [];
	let url: SyntaxNode | null = null;
	let title: SyntaxNode | null = null;
	for (let child = node.firstChild; child; child = child.nextSibling) {
		if (child.name === 'LinkMark') marks.push(child);
		else if (child.name === 'URL') url = child;
		else if (child.name === 'LinkTitle') title = child;
	}
	// `[label](url)`: marks are `[` (or `![`), `]`, `(`, `)`.
	const open = marks[0];
	const close = marks[1];
	if (!open || !close || !url) return null;
	const target = state.sliceDoc(url.from, url.to).replace(/^<|>$/g, '');
	const titleText = title
		? state.sliceDoc(title.from + 1, title.to - 1)
		: undefined;
	return {
		labelFrom: open.to,
		labelTo: close.from,
		label: state.sliceDoc(open.to, close.from),
		target,
		title: titleText,
	};
}

function decorateLink(cx: DecorationContext, node: SyntaxNode) {
	const { state, addMark, addReplace, revealed, widgets } = cx;
	const parts = linkParts(state, node);
	if (!parts) return;
	const image = node.name === 'Image';
	if (isPdfTarget(parts.target)) {
		const line = state.doc.lineAt(node.from);
		const raw = state.sliceDoc(node.from, node.to);
		const block = image && line.text.trim() === raw;
		const widget = widgets.link(
			parts.target,
			parts.label || parts.target,
			block,
		);
		if (widget && !revealed(node.from, node.to)) {
			addReplace(block ? line.from : node.from, block ? line.to : node.to, {
				widget,
				block,
			});
		}
		return false;
	}
	if (image) {
		const widget = widgets.image(parts.label, parts.target, {
			title: parts.title,
		});
		if (widget && !revealed(node.from, node.to)) {
			addReplace(node.from, node.to, { widget });
		} else if (widget) {
			addMark(node.from, node.to, 'cm-md-image-active');
		}
		return false;
	}
	const attributes = {
		'data-href': parts.target,
		'data-link-kind': 'markdown',
	};
	if (revealed(node.from, node.to)) {
		addMark(parts.labelFrom, parts.labelTo, 'cm-md-link', attributes);
		return;
	}
	if (parts.labelTo <= parts.labelFrom) return;
	addReplace(node.from, parts.labelFrom, {});
	addMark(
		parts.labelFrom,
		parts.labelTo,
		'cm-md-link cm-md-link-rendered',
		attributes,
	);
	addReplace(parts.labelTo, node.to, {});
}

function decorateTable(cx: DecorationContext, node: SyntaxNode) {
	const { state, addLine, addReplace, revealed, eachLine, widgets } = cx;
	const startLine = state.doc.lineAt(node.from);
	const endLine = state.doc.lineAt(node.to - 1);
	if (!revealed(node.from, node.to)) {
		// Block widgets must cover whole lines and may only be provided
		// through state. Skip the subtree: content inside the replaced range
		// cannot render.
		const source = state.sliceDoc(startLine.from, endLine.to);
		addReplace(startLine.from, endLine.to, {
			widget: widgets.table(source, parseTableRows(source)),
			block: true,
		});
		return false;
	}
	eachLine(node.from, node.to, (from) => addLine(from, 'cm-md-table-row'));
	return;
}

/**
 * Merge the collected decorations into one legal range set. Replace ranges
 * are flattened first (outermost wins), then anything strictly inside a
 * surviving replace range is dropped, because content inside a replaced
 * range never renders. Decorations that merely touch a replace boundary
 * (line classes at the line start, marks crossing the edge) survive.
 */
function finishDecorationSet(pushed: CollectedDecoration[]): DecorationSet {
	const replacements = pushed
		.filter((item) => item.replaces)
		.sort((a, b) => a.from - b.from || b.to - a.to);
	const accepted: CollectedDecoration[] = [];
	for (const candidate of replacements) {
		const last = accepted[accepted.length - 1];
		// Sorted by start and disjoint, so only the last accepted range can
		// overlap the candidate.
		if (!last || candidate.from >= last.to) accepted.push(candidate);
	}
	const inside = (item: CollectedDecoration) => {
		// Binary search the accepted (sorted, disjoint) replace ranges.
		let low = 0;
		let high = accepted.length - 1;
		while (low <= high) {
			const middle = (low + high) >> 1;
			const replacement = accepted[middle]!;
			if (replacement.to < item.from) low = middle + 1;
			else if (replacement.from > item.to) high = middle - 1;
			else {
				for (
					let index = Math.max(0, middle - 1);
					index <= Math.min(accepted.length - 1, middle + 1);
					index += 1
				) {
					const candidate = accepted[index]!;
					if (item.to > candidate.to || item.from < candidate.from) continue;
					// Zero-length decorations at the replace start decorate the
					// line itself and still render before the widget.
					if (item.from === item.to && item.from === candidate.from) continue;
					return true;
				}
				return false;
			}
		}
		return false;
	};
	const ranges = accepted
		.concat(pushed.filter((item) => !item.replaces && !inside(item)))
		.map((item) => item.deco.range(item.from, item.to));
	return RangeSet.of(ranges, true);
}

/** Cells of a pipe table, with offsets relative to the table source. */
function parseTableRows(source: string): TableCell[][] {
	let cursor = 0;
	return source.split('\n').map((line) => {
		const rowStart = cursor;
		cursor += line.length + 1;
		const cells: TableCell[] = [];
		let start = line.startsWith('|') ? 1 : 0;
		let escaped = false;
		for (let index = start; index <= line.length; index += 1) {
			const character = line[index];
			if (character === '\\' && !escaped) {
				escaped = true;
				continue;
			}
			if ((character === '|' && !escaped) || index === line.length) {
				if (index > start) {
					cells.push({
						text: line.slice(start, index),
						from: rowStart + start,
						to: rowStart + index,
					});
				}
				start = index + 1;
			}
			escaped = false;
		}
		return cells;
	});
}

function literalBlockAt(state: EditorState, from: number, to: number) {
	const tree = syntaxTree(state);
	for (const position of [from, Math.min(to, from + 1)]) {
		let node: SyntaxNode | null = tree.resolveInner(position, 1);
		while (node) {
			if (LITERAL_BLOCKS.has(node.name)) return true;
			node = node.parent;
		}
	}
	return false;
}

function inlineCodeSpans(
	state: EditorState,
	from: number,
	to: number,
): Array<{ from: number; to: number }> {
	const spans: Array<{ from: number; to: number }> = [];
	syntaxTree(state).iterate({
		from,
		to,
		enter(node) {
			if (node.name === 'InlineCode' || node.name === 'URL') {
				spans.push({ from: node.from, to: node.to });
			}
		},
	});
	return spans;
}

/** Obsidian `[[target#heading|alias]]` parts. */
function wikilinkParts(inner: string) {
	const pipe = inner.indexOf('|');
	const target = (pipe >= 0 ? inner.slice(0, pipe) : inner).trim();
	const alias = pipe >= 0 ? inner.slice(pipe + 1).trim() : undefined;
	return { target, alias };
}

function decorateObsidianSyntax(
	cx: DecorationContext,
	from: number,
	to: number,
) {
	const { state, widgets, addLine, addMark, addReplace, revealed } = cx;
	const first = state.doc.lineAt(from).number;
	const last = state.doc.lineAt(Math.max(from, to - 1)).number;
	for (let number = first; number <= last; number += 1) {
		const line = state.doc.line(number);
		if (literalBlockAt(state, line.from, line.to)) continue;
		// Obsidian-style matches are found on raw line text, so skip anything
		// inside code spans and URLs, where the syntax is meant to be literal.
		const codeSpans = inlineCodeSpans(state, line.from, line.to);
		const inCode = (start: number, end: number) =>
			codeSpans.some((span) => start < span.to && end > span.from);
		const task = taskMarker(line.text);
		if (task && widgets.checkbox) {
			const boxFrom = line.from + task.boxFrom;
			const boxTo = line.from + task.boxTo;
			addLine(line.from, 'cm-md-task-line');
			if (task.checked && boxTo < line.to) {
				addMark(boxTo, line.to, 'cm-md-task-done');
			}
			if (!revealed(line.from + task.markerFrom, boxTo)) {
				addReplace(task.bullet ? line.from + task.markerFrom : boxFrom, boxTo, {
					widget: widgets.checkbox(task.checked),
				});
			}
		}
		for (const match of line.text.matchAll(/(!)?\[\[([^\]]+)\]\]/g)) {
			const start = line.from + (match.index ?? 0);
			const end = start + match[0].length;
			if (inCode(start, end)) continue;
			const { target, alias } = wikilinkParts(match[2] ?? '');
			const requestedEmbed = Boolean(match[1]);
			const standalone = line.text.trim() === match[0];
			if (requestedEmbed && isImageTarget(target)) {
				const width =
					alias && /^\d+(x\d+)?$/.test(alias)
						? Number.parseInt(alias, 10)
						: undefined;
				const widget = widgets.image(
					width ? target : (alias ?? target),
					target,
					{
						width,
					},
				);
				if (widget && !revealed(start, end)) addReplace(start, end, { widget });
				else addMark(start, end, 'cm-md-embed');
				continue;
			}
			const embed = requestedEmbed && (!isPdfTarget(target) || standalone);
			const widget = widgets.link(target, alias ?? target, embed);
			if (widget && !revealed(start, end)) {
				addReplace(
					embed && standalone ? line.from : start,
					embed && standalone ? line.to : end,
					{ widget, block: embed && standalone },
				);
			} else {
				addMark(start, end, embed ? 'cm-md-embed' : 'cm-md-wikilink', {
					'data-href': target,
					'data-link-kind': 'wiki',
				});
			}
		}
		for (const match of line.text.matchAll(
			/(^|\s)(#[\p{L}\p{N}_/-]*[\p{L}_/-][\p{L}\p{N}_/-]*)/gu,
		)) {
			const prefix = match[1] ?? '';
			const tag = match[2] ?? '';
			const start = line.from + (match.index ?? 0) + prefix.length;
			if (inCode(start, start + tag.length)) continue;
			addMark(start, start + tag.length, 'cm-md-tag');
		}
		for (const match of line.text.matchAll(/(^|\s)\^([\w-]+)$/g)) {
			const start = line.from + (match.index ?? 0) + (match[1] ?? '').length;
			if (inCode(start, line.to)) continue;
			addMark(start, line.to, 'cm-md-block-id');
		}
		for (const match of line.text.matchAll(/==([^=]+)==/g)) {
			const start = line.from + (match.index ?? 0);
			const end = start + match[0].length;
			if (inCode(start, end)) continue;
			addMark(start, end, 'cm-md-highlight');
			if (!revealed(start, end)) {
				addReplace(start, start + 2, {});
				addReplace(end - 2, end, {});
			}
		}
		for (const match of line.text.matchAll(/\[\^[^\]]+\]/g)) {
			const start = line.from + (match.index ?? 0);
			if (inCode(start, start + match[0].length)) continue;
			addMark(start, start + match[0].length, 'cm-md-footnote');
		}
		for (const match of line.text.matchAll(/<u>(.*?)<\/u>/g)) {
			const start = line.from + (match.index ?? 0);
			if (inCode(start, start + match[0].length)) continue;
			addMark(start, start + match[0].length, 'cm-md-underline');
		}
		for (const match of line.text.matchAll(/%%(.*?)%%/g)) {
			const start = line.from + (match.index ?? 0);
			const end = start + match[0].length;
			if (inCode(start, end)) continue;
			if (!revealed(start, end)) addReplace(start, end, {});
		}
		for (const match of inlineMathMatches(line.text)) {
			const start = line.from + match.index;
			const end = start + match.text.length;
			if (inCode(start, end)) continue;
			if (!revealed(start, end)) {
				addReplace(start, end, {
					widget: widgets.math(match.source, false),
				});
			}
		}
	}
}
