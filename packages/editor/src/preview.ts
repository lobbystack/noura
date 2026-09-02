import { syntaxTree } from '@codemirror/language';
import type { Decoration, DecorationSet, WidgetType } from '@codemirror/view';
import { RangeSet, type EditorState } from '@codemirror/state';
import type { SyntaxNode } from '@lezer/common';

/** One visible document span. Mirrors `EditorView.visibleRanges`. */
export interface PreviewViewportRange {
	from: number;
	to: number;
}

export interface PreviewWidgetFactories {
	image: (alt: string, src: string) => WidgetType | null;
	math: (source: string, displayMode: boolean) => WidgetType;
	table: (
		rows: Array<Array<{ text: string; from: number; to: number }>>,
	) => WidgetType;
	link: (
		target: string,
		label: string,
		embed: boolean,
		from: number,
		to: number,
	) => WidgetType | null;
}

function selectionTouches(
	state: EditorState,
	from: number,
	to: number,
): boolean {
	return state.selection.ranges.some(
		(range) => range.from <= to && range.to >= from,
	);
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

/**
 * One pass over the whole state: syntax-tree marks and line classes for
 * inline styling, plus replace-decorations for checkboxes, images, tables,
 * math, and Obsidian syntax. Cursor-adjacent constructs stay as raw syntax
 * so editing never fights the editor.
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
	const addMark = (from: number, to: number, cls: string) => {
		if (to > from) {
			pushed.push({
				from,
				to,
				deco: deco.mark({ class: cls }),
				replaces: false,
			});
		}
	};
	const addLine = (lineFrom: number, cls: string) => {
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
		pushed.push({ from, to, deco: deco.replace(spec), replaces: true });
	};
	for (const visible of viewport) {
		syntaxTree(state).iterate({
			from: visible.from,
			to: visible.to,
			enter(node: SyntaxNode): boolean | void {
				const heading = headingLineClass(node);
				if (heading) {
					const line = state.doc.lineAt(node.from);
					addLine(line.from, heading);
					// Hide the # marker when the cursor is elsewhere.
					const match = line.text.match(/^(#{1,6})\s+/);
					if (match && !selectionTouches(state, node.from, line.to)) {
						const markerEnd = line.from + match[0].length;
						if (markerEnd > line.from) {
							addReplace(line.from, markerEnd, {});
						}
					}
					return;
				}
				if (
					node.name === 'EmphasisMark' ||
					node.name === 'StrongEmphasisMark' ||
					node.name === 'StrikethroughMark' ||
					node.name === 'CodeMark'
				) {
					if (!selectionTouches(state, node.from - 40, node.to + 40)) {
						addReplace(node.from, node.to, {});
					}
					return;
				}
				if (node.name === 'Emphasis') {
					addMark(node.from, node.to, 'cm-md-emphasis');
					return;
				}
				if (node.name === 'StrongEmphasis') {
					addMark(node.from, node.to, 'cm-md-strong');
					return;
				}
				if (node.name === 'Strikethrough') {
					addMark(node.from, node.to, 'cm-md-strike');
					return;
				}
				if (node.name === 'InlineCode') {
					addMark(node.from, node.to, 'cm-md-code');
					return;
				}
				if (node.name === 'Blockquote') {
					const line = state.doc.lineAt(node.from);
					addLine(line.from, 'cm-md-quote-line');
					return;
				}
				if (node.name === 'Image') {
					const inner = state.sliceDoc(node.from, node.to);
					const match = inner.match(/^!\[([^\]]*)\]\(([^)]*)\)$/);
					if (match) {
						const widget = widgets.image(match[1] ?? '', match[2] ?? '');
						if (
							widget &&
							!selectionTouches(state, node.from - 30, node.to + 30)
						) {
							addReplace(node.from, node.to, { widget });
						} else if (widget) {
							addMark(node.from, node.to, 'cm-md-image-active');
						}
					}
					return;
				}
				if (node.name === 'Task') {
					const line = state.doc.lineAt(node.from);
					const checked = /\[[xX]\]/.test(line.text);
					addLine(line.from, 'cm-md-task-line');
					addMark(
						node.from,
						node.to,
						checked ? 'cm-md-task-done' : 'cm-md-task-open',
					);
					return;
				}
				if (node.name === 'ListMark' || node.name === 'QuoteMark') {
					addMark(node.from, node.to, 'cm-md-list-mark');
					return;
				}
				if (node.name === 'URL') {
					addMark(node.from, node.to, 'cm-md-url');
					return;
				}
				if (node.name === 'HorizontalRule') {
					const line = state.doc.lineAt(node.from);
					addLine(line.from, 'cm-md-hr-line');
					return;
				}
				if (node.name === 'Table') {
					if (!selectionTouches(state, node.from, node.to)) {
						const rows = parseTableRows(
							state.sliceDoc(node.from, node.to),
							node.from,
						);
						// Block widgets must cover whole lines and may only be
						// provided through state. Skip the subtree: content inside
						// the replaced range cannot render.
						const startLine = state.doc.lineAt(node.from);
						const endLine = state.doc.lineAt(node.to - 1);
						addReplace(startLine.from, endLine.to, {
							widget: widgets.table(rows),
							block: true,
						});
						return false;
					}
					const line = state.doc.lineAt(node.from);
					const endLine = state.doc.lineAt(node.to - 1);
					for (
						let number = line.number;
						number <= endLine.number;
						number += 1
					) {
						const lineInfo = state.doc.line(number);
						addLine(lineInfo.from, 'cm-md-table-row');
					}
					return;
				}
			},
		});
		decorateObsidianSyntax(
			state,
			visible.from,
			visible.to,
			widgets,
			pushed,
			addLine,
			addMark,
			addReplace,
		);
	}
	return finishDecorationSet(pushed);
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
		if (
			accepted.every(
				(kept) => candidate.to <= kept.from || candidate.from >= kept.to,
			)
		) {
			accepted.push(candidate);
		}
	}
	const ranges = accepted
		.concat(
			pushed.filter(
				(item) =>
					!item.replaces &&
					!accepted.some((replacement) => {
						if (item.to > replacement.to || item.from < replacement.from) {
							return false;
						}
						// Zero-length decorations at the replace start decorate the
						// line itself and still render before the widget.
						return !(item.from === item.to && item.from === replacement.from);
					}),
			),
		)
		.map((item) => item.deco.range(item.from, item.to));
	return RangeSet.of(ranges, true);
}
function parseTableRows(source: string, offset: number) {
	let cursor = offset;
	return source.split('\n').map((line) => {
		const rowStart = cursor;
		cursor += line.length + 1;
		const cells: Array<{ text: string; from: number; to: number }> = [];
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

function decorateObsidianSyntax(
	state: EditorState,
	from: number,
	to: number,
	widgets: Pick<PreviewWidgetFactories, 'math' | 'link'>,
	pushed: CollectedDecoration[],
	addLine: (from: number, cls: string) => void,
	addMark: (from: number, to: number, cls: string) => void,
	addReplace: (
		from: number,
		to: number,
		spec: { widget?: WidgetType; block?: boolean } | Record<string, never>,
	) => void,
) {
	const first = state.doc.lineAt(from).number;
	const last = state.doc.lineAt(Math.max(from, to - 1)).number;
	for (let number = first; number <= last; number += 1) {
		const line = state.doc.line(number);
		if (isBlockCodeLine(state, line.from, line.to)) continue;
		if (/^\s*>\s*\[![^\]]+\]/.test(line.text))
			addLine(line.from, 'cm-md-callout');
		// Obsidian-style matches are found on raw line text, so skip anything
		// inside code spans, where the syntax is meant to be literal.
		const codeSpans = inlineCodeSpans(state, line.from, line.to);
		const inCode = (start: number, end: number) =>
			codeSpans.some((span) => start < span.to && end > span.from);
		for (const match of line.text.matchAll(/(!)?\[\[([^\]]+)\]\]/g)) {
			const start = line.from + (match.index ?? 0);
			const end = start + match[0].length;
			if (inCode(start, end)) continue;
			const [target = '', alias] = (match[2] ?? '').split('|', 2);
			const embed = Boolean(match[1]);
			const standalone = line.text.trim() === match[0];
			const widget = widgets.link(target, alias ?? target, embed, start, end);
			if (widget && !selectionTouches(state, start, end)) {
				addReplace(
					embed && standalone ? line.from : start,
					embed && standalone ? line.to : end,
					{ widget, block: embed && standalone },
				);
			} else {
				addMark(start, end, embed ? 'cm-md-embed' : 'cm-md-wikilink');
			}
		}
		for (const match of line.text.matchAll(/(^|\s)(#[\p{L}\p{N}_/-]+)/gu)) {
			const prefix = match[1] ?? '';
			const tag = match[2] ?? '';
			const start = line.from + (match.index ?? 0) + prefix.length;
			if (inCode(start, start + tag.length)) continue;
			addMark(start, start + tag.length, 'cm-md-tag');
		}
		for (const match of line.text.matchAll(/\^([\w-]+)$/g)) {
			const start = line.from + (match.index ?? 0);
			if (inCode(start, start + match[0].length)) continue;
			addMark(start, start + match[0].length, 'cm-md-block-id');
		}
		for (const match of line.text.matchAll(/==([^=]+)==/g)) {
			const start = line.from + (match.index ?? 0);
			if (inCode(start, start + match[0].length)) continue;
			addMark(start, start + match[0].length, 'cm-md-highlight');
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
			if (!selectionTouches(state, start, end)) addReplace(start, end, {});
		}
		const block = line.text.match(/^\s*\$\$(.*?)\$\$\s*$/);
		if (block && !selectionTouches(state, line.from, line.to)) {
			addReplace(line.from, line.to, {
				widget: widgets.math(block[1] ?? '', true),
				block: true,
			});
			continue;
		}
		for (const match of inlineMathMatches(line.text)) {
			const start = line.from + match.index;
			const end = start + match.text.length;
			if (inCode(start, end)) continue;
			if (!selectionTouches(state, start, end)) {
				addReplace(start, end, {
					widget: widgets.math(match.source, false),
				});
			}
		}
	}
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
		enter(node: SyntaxNode) {
			if (node.name === 'InlineCode') {
				spans.push({ from: node.from, to: node.to });
			}
		},
	});
	return spans;
}

function isBlockCodeLine(state: EditorState, from: number, to: number) {
	const tree = syntaxTree(state);
	for (const position of [from, Math.min(to, from + 1)]) {
		let node: SyntaxNode | null = tree.resolveInner(position, 1);
		while (node) {
			if (node.name === 'FencedCode' || node.name === 'CodeBlock') return true;
			node = node.parent;
		}
	}
	return false;
}
