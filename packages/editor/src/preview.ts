import { syntaxTree } from '@codemirror/language';
import type { Decoration, DecorationSet, EditorView } from '@codemirror/view';
import { RangeSetBuilder } from '@codemirror/state';
import type { SyntaxNode } from '@lezer/common';

function selectionTouches(view: EditorView, from: number, to: number): boolean {
	return view.state.selection.ranges.some(
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

/**
 * One pass over the visible syntax tree. Emits marks for inline styling,
 * line classes for headings, quote prefixes, and replace-decorations for
 * checkboxes and images. Cursor-adjacent constructs stay as raw syntax so
 * editing never fights the editor.
 */
export function buildDecorations(
	view: EditorView,
	deco: typeof Decoration,
	imageWidget: (
		alt: string,
		src: string,
	) => import('@codemirror/view').WidgetType | null,
): DecorationSet {
	const builder = new RangeSetBuilder<Decoration>();
	const pushed: Array<{ from: number; to: number; deco: Decoration }> = [];
	const addMark = (from: number, to: number, cls: string) => {
		if (to > from) pushed.push({ from, to, deco: deco.mark({ class: cls }) });
	};
	const addLine = (lineFrom: number, cls: string) => {
		pushed.push({
			from: lineFrom,
			to: lineFrom,
			deco: deco.line({ class: cls }),
		});
	};
	for (const visible of view.visibleRanges) {
		syntaxTree(view.state).iterate({
			from: visible.from,
			to: visible.to,
			enter(node: SyntaxNode) {
				const heading = headingLineClass(node);
				if (heading) {
					const line = view.state.doc.lineAt(node.from);
					addLine(line.from, heading);
					// Hide the # marker when the cursor is elsewhere.
					const match = line.text.match(/^(#{1,6})\s+/);
					if (match && !selectionTouches(view, node.from, line.to)) {
						const markerEnd = line.from + match[0].length;
						if (markerEnd > line.from) {
							pushed.push({
								from: line.from,
								to: markerEnd,
								deco: deco.replace({}),
							});
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
					if (!selectionTouches(view, node.from - 40, node.to + 40)) {
						pushed.push({
							from: node.from,
							to: node.to,
							deco: deco.replace({}),
						});
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
					const line = view.state.doc.lineAt(node.from);
					addLine(line.from, 'cm-md-quote-line');
					return;
				}
				if (node.name === 'Image') {
					const inner = view.state.sliceDoc(node.from, node.to);
					const match = inner.match(/^!\[([^\]]*)\]\(([^)]*)\)$/);
					if (match) {
						const widget = imageWidget(match[1] ?? '', match[2] ?? '');
						if (
							widget &&
							!selectionTouches(view, node.from - 30, node.to + 30)
						) {
							pushed.push({
								from: node.from,
								to: node.to,
								deco: deco.replace({ widget }),
							});
						} else if (widget) {
							addMark(node.from, node.to, 'cm-md-image-active');
						}
					}
					return;
				}
				if (node.name === 'Task') {
					const line = view.state.doc.lineAt(node.from);
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
					const line = view.state.doc.lineAt(node.from);
					addLine(line.from, 'cm-md-hr-line');
					return;
				}
				if (node.name === 'Table') {
					const line = view.state.doc.lineAt(node.from);
					const endLine = view.state.doc.lineAt(node.to - 1);
					for (
						let number = line.number;
						number <= endLine.number;
						number += 1
					) {
						const lineInfo = view.state.doc.line(number);
						addLine(lineInfo.from, 'cm-md-table-row');
					}
					return;
				}
			},
		});
	}
	pushed.sort((a, b) => a.from - b.from || a.to - b.to);
	for (const item of pushed) {
		builder.add(item.from, item.to, item.deco);
	}
	return builder.finish();
}
