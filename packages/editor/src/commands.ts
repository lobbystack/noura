import {
	EditorSelection,
	type ChangeSpec,
	type EditorState,
	type SelectionRange,
	type TransactionSpec,
} from '@codemirror/state';
import type { KeyBinding } from '@codemirror/view';
import { syntaxTree } from '@codemirror/language';
import { taskMarker } from './preview';
import type { MarkdownFormat } from './types';

/** What the commands need from a view; tests pass a DOM-free stand-in. */
export interface CommandView {
	readonly state: EditorState;
	dispatch(spec: TransactionSpec): void;
}

type InlineKind = 'bold' | 'italic' | 'strikethrough' | 'code';

const INLINE: Record<
	InlineKind,
	{ chars: string[]; insert: string; wrapped: (run: number) => boolean }
> = {
	// A run of two asterisks is bold, one is italic, three is both.
	bold: { chars: ['*', '_'], insert: '**', wrapped: (run) => run >= 2 },
	italic: { chars: ['*', '_'], insert: '*', wrapped: (run) => run % 2 === 1 },
	strikethrough: { chars: ['~'], insert: '~~', wrapped: (run) => run >= 2 },
	code: { chars: ['`'], insert: '`', wrapped: (run) => run >= 1 },
};

function runBefore(state: EditorState, pos: number, char: string) {
	let count = 0;
	while (
		pos - count > 0 &&
		state.sliceDoc(pos - count - 1, pos - count) === char
	)
		count += 1;
	return count;
}

function runAfter(state: EditorState, pos: number, char: string) {
	let count = 0;
	const length = state.doc.length;
	while (
		pos + count < length &&
		state.sliceDoc(pos + count, pos + count + 1) === char
	)
		count += 1;
	return count;
}

function leadingRun(text: string, char: string) {
	let count = 0;
	while (text[count] === char) count += 1;
	return count;
}

function trailingRun(text: string, char: string) {
	let count = 0;
	while (text[text.length - 1 - count] === char) count += 1;
	return count;
}

/**
 * Selections usually cover only the visible text because live preview hides
 * the markers. When the markers were selected too, move them outside the
 * range so both cases toggle the same way.
 */
function normalizeInline(
	state: EditorState,
	range: SelectionRange,
	char: string,
): { from: number; to: number } {
	const text = state.sliceDoc(range.from, range.to);
	const run = Math.min(leadingRun(text, char), trailingRun(text, char));
	if (run === 0 || run * 2 >= text.length) return range;
	const inner = text.slice(run, text.length - run);
	if (inner.includes(char)) return range;
	return { from: range.from + run, to: range.to - run };
}

function toggleInline(view: CommandView, kind: InlineKind) {
	const spec = INLINE[kind];
	const state = view.state;
	view.dispatch(
		state.changeByRange((range) => {
			for (const char of spec.chars) {
				const { from, to } = normalizeInline(state, range, char);
				const before = runBefore(state, from, char);
				const after = runAfter(state, to, char);
				if (
					before > 0 &&
					after > 0 &&
					spec.wrapped(before) &&
					spec.wrapped(after)
				) {
					const size = spec.insert.length;
					return {
						changes: [
							{ from: from - size, to: from },
							{ from: to, to: to + size },
						],
						range: EditorSelection.range(from - size, to - size),
					};
				}
			}
			const size = spec.insert.length;
			return {
				changes: [
					{ from: range.from, insert: spec.insert },
					{ from: range.to, insert: spec.insert },
				],
				range: EditorSelection.range(range.from + size, range.to + size),
			};
		}),
	);
}

function togglePair(view: CommandView, opening: string, closing: string) {
	const state = view.state;
	view.dispatch(
		state.changeByRange((range) => {
			const text = state.sliceDoc(range.from, range.to);
			if (
				text.length >= opening.length + closing.length &&
				text.startsWith(opening) &&
				text.endsWith(closing)
			) {
				return {
					changes: {
						from: range.from,
						to: range.to,
						insert: text.slice(opening.length, text.length - closing.length),
					},
					range: EditorSelection.range(
						range.from,
						range.to - opening.length - closing.length,
					),
				};
			}
			if (
				state.sliceDoc(range.from - opening.length, range.from) === opening &&
				state.sliceDoc(range.to, range.to + closing.length) === closing
			) {
				return {
					changes: [
						{ from: range.from - opening.length, to: range.from },
						{ from: range.to, to: range.to + closing.length },
					],
					range: EditorSelection.range(
						range.from - opening.length,
						range.to - opening.length,
					),
				};
			}
			return {
				changes: [
					{ from: range.from, insert: opening },
					{ from: range.to, insert: closing },
				],
				range: EditorSelection.range(
					range.from + opening.length,
					range.to + opening.length,
				),
			};
		}),
	);
}

const LINE_PREFIX =
	/^(\s*)(?:([-*+])|(\d+)([.)]))(\s+)(\[[ xX]\](?:\s+|$))?|^(\s*)(>\s?)/;

type LineKind = 'bulletList' | 'numberedList' | 'checkList' | 'blockQuote';

interface LineShape {
	indent: string;
	/** Length of the list or quote prefix, including the indent. */
	prefixLength: number;
	kind: LineKind | null;
}

function lineShape(text: string): LineShape {
	const match = LINE_PREFIX.exec(text);
	if (!match) {
		const indent = /^\s*/.exec(text)?.[0] ?? '';
		return { indent, prefixLength: indent.length, kind: null };
	}
	if (match[8] !== undefined) {
		return {
			indent: match[7] ?? '',
			prefixLength: match[0].length,
			kind: 'blockQuote',
		};
	}
	return {
		indent: match[1] ?? '',
		prefixLength: match[0].length,
		kind: match[6] ? 'checkList' : match[2] ? 'bulletList' : 'numberedList',
	};
}

function selectedLines(state: EditorState): number[] {
	const lines = new Set<number>();
	for (const range of state.selection.ranges) {
		const first = state.doc.lineAt(range.from).number;
		const last = state.doc.lineAt(range.to).number;
		for (let number = first; number <= last; number += 1) {
			// Blank lines inside a multi-line selection keep no marker.
			if (first !== last && state.doc.line(number).text.trim() === '') continue;
			lines.add(number);
		}
	}
	return [...lines].sort((left, right) => left - right);
}

/**
 * Turn the selected lines into one kind of list or quote, or back into
 * plain paragraphs when they already all are that kind.
 */
function toggleLines(view: CommandView, kind: LineKind) {
	const state = view.state;
	const lines = selectedLines(state);
	const shapes = lines.map((number) => lineShape(state.doc.line(number).text));
	const remove = shapes.every((shape) => shape.kind === kind);
	const changes: ChangeSpec[] = [];
	lines.forEach((number, index) => {
		const line = state.doc.line(number);
		const shape = shapes[index]!;
		const from = line.from + shape.indent.length;
		const to = line.from + shape.prefixLength;
		if (remove) {
			changes.push({ from, to });
			return;
		}
		if (shape.kind === kind) return;
		// Quotes wrap whatever the line is; lists replace another list marker.
		if (kind === 'blockQuote') {
			changes.push({ from, insert: '> ' });
			return;
		}
		const replaced = shape.kind === 'blockQuote' ? from : to;
		const insert =
			kind === 'bulletList'
				? '- '
				: kind === 'numberedList'
					? `${index + 1}. `
					: shape.kind === 'numberedList'
						? `${lineMarker(line.text)}[ ] `
						: '- [ ] ';
		changes.push({ from, to: replaced, insert });
	});
	view.dispatch({ changes });
}

function lineMarker(text: string) {
	const match = /^\s*(\d+[.)]\s+)/.exec(text);
	return match?.[1] ?? '- ';
}

function setHeading(view: CommandView, level: 0 | 1 | 2 | 3) {
	const marker = level === 0 ? '' : '#'.repeat(level) + ' ';
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	const seen = new Set<number>();
	for (const range of view.state.selection.ranges) {
		const lineInfo = view.state.doc.lineAt(range.from);
		if (seen.has(lineInfo.number)) continue;
		seen.add(lineInfo.number);
		const withoutHashes = lineInfo.text.replace(/^#{1,6}\s+/, '');
		const replacement = level === 0 ? withoutHashes : marker + withoutHashes;
		if (replacement !== lineInfo.text) {
			changes.push({
				from: lineInfo.from,
				to: lineInfo.from + lineInfo.text.length,
				insert: replacement,
			});
		}
	}
	view.dispatch({ changes });
}

/** Flip the checkbox on each given line. Lines without one are left alone. */
export function toggleCheckboxes(view: CommandView, lines: number[]): void {
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	for (const lineNumber of lines) {
		const lineInfo = view.state.doc.line(lineNumber);
		const task = taskMarker(lineInfo.text);
		if (!task) continue;
		changes.push({
			from: lineInfo.from + task.boxFrom,
			to: lineInfo.from + task.boxTo,
			insert: task.checked ? '[ ]' : '[x]',
		});
	}
	view.dispatch({ changes });
}

/**
 * Mod-Enter: check or uncheck a task, or turn the line into a task first,
 * like Obsidian's "Toggle checkbox status".
 */
export function toggleTask(view: CommandView): boolean {
	const state = view.state;
	const lines = selectedLines(state);
	const changes: ChangeSpec[] = [];
	for (const number of lines) {
		const line = state.doc.line(number);
		const task = taskMarker(line.text);
		if (task) {
			changes.push({
				from: line.from + task.boxFrom,
				to: line.from + task.boxTo,
				insert: task.checked ? '[ ]' : '[x]',
			});
			continue;
		}
		const shape = lineShape(line.text);
		if (shape.kind === 'bulletList' || shape.kind === 'numberedList') {
			changes.push({ from: line.from + shape.prefixLength, insert: '[ ] ' });
		} else {
			changes.push({ from: line.from + shape.indent.length, insert: '- [ ] ' });
		}
	}
	view.dispatch({ changes });
	return true;
}

const URL_LIKE = /^(?:[a-z][a-z\d+.-]*:\/\/|mailto:|www\.)\S+$/i;

function insertLink(view: CommandView) {
	const state = view.state;
	view.dispatch(
		state.changeByRange((range) => {
			const text = state.sliceDoc(range.from, range.to);
			if (URL_LIKE.test(text)) {
				// A selected URL becomes the target; type the label next.
				return {
					changes: { from: range.from, to: range.to, insert: `[](${text})` },
					range: EditorSelection.cursor(range.from + 1),
				};
			}
			// Selected text becomes the label; type the target next.
			return {
				changes: { from: range.from, to: range.to, insert: `[${text}]()` },
				range: EditorSelection.cursor(
					range.from + text.length + (text ? 3 : 1),
				),
			};
		}),
	);
}

const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])(\s+)/;

function indentWidth(text: string) {
	return /^\s*/.exec(text)?.[0] ?? '';
}

function visualWidth(whitespace: string) {
	let width = 0;
	for (const character of whitespace) width += character === '\t' ? 4 : 1;
	return width;
}

/** The item a list line belongs under, searching upward. */
function previousItem(state: EditorState, lineNumber: number, below: number) {
	for (let number = lineNumber - 1; number >= 1; number -= 1) {
		const text = state.doc.line(number).text;
		if (text.trim() === '') continue;
		const match = LIST_ITEM.exec(text);
		const width = visualWidth(indentWidth(text));
		if (!match) {
			// Paragraph continuation lines are indented under their item.
			if (width > 0) continue;
			return null;
		}
		if (width <= below) return { text, match, width };
	}
	return null;
}

/** The line range of a list item plus its nested children. */
function itemBlock(state: EditorState, lineNumber: number) {
	const width = visualWidth(indentWidth(state.doc.line(lineNumber).text));
	let last = lineNumber;
	for (let number = lineNumber + 1; number <= state.doc.lines; number += 1) {
		const text = state.doc.line(number).text;
		if (text.trim() === '') continue;
		if (visualWidth(indentWidth(text)) <= width) break;
		last = number;
	}
	return last;
}

function listLines(state: EditorState) {
	const lines = new Set<number>();
	for (const range of state.selection.ranges) {
		const first = state.doc.lineAt(range.from).number;
		const last = state.doc.lineAt(range.to).number;
		for (let number = first; number <= last; number += 1) lines.add(number);
	}
	const sorted = [...lines].sort((left, right) => left - right);
	if (!sorted.every((number) => LIST_ITEM.test(state.doc.line(number).text)))
		return null;
	return sorted;
}

/**
 * Tab on a list item nests it under the item above, aligned with that item's
 * text so ordered lists nest too (two spaces are not enough under `1. `).
 * Nested children move with their item. Returns false outside lists so Tab
 * keeps its ordinary meaning.
 */
export function indentListItem(view: CommandView): boolean {
	const state = view.state;
	const lines = listLines(state);
	if (!lines) return false;
	const changes: ChangeSpec[] = [];
	const moved = new Set<number>();
	for (const number of lines) {
		if (moved.has(number)) continue;
		const line = state.doc.line(number);
		const own = indentWidth(line.text);
		const sibling = previousItem(state, number, visualWidth(own));
		if (!sibling || sibling.width !== visualWidth(own)) continue;
		const siblingIndent = indentWidth(sibling.text);
		const target =
			siblingIndent.includes('\t') || own.includes('\t')
				? `${siblingIndent}\t`
				: ' '.repeat(sibling.match[0].length);
		const delta = visualWidth(target) - visualWidth(own);
		if (delta <= 0) continue;
		const last = itemBlock(state, number);
		for (let child = number; child <= last; child += 1) {
			moved.add(child);
			const childLine = state.doc.line(child);
			if (childLine.text.trim() === '') continue;
			const childIndent = indentWidth(childLine.text);
			const nextIndent =
				child === number ? target : target + childIndent.slice(own.length);
			changes.push({
				from: childLine.from,
				to: childLine.from + childIndent.length,
				insert: nextIndent,
			});
		}
		// A nested ordered item starts its own numbering.
		const ordered = /^(\s*)(\d+)([.)])/.exec(line.text);
		if (ordered && /^\d/.test(sibling.match[2] ?? '')) {
			const numberFrom = line.from + (ordered[1] ?? '').length;
			changes.push({
				from: numberFrom,
				to: numberFrom + (ordered[2] ?? '').length,
				insert: '1',
			});
		}
	}
	if (changes.length === 0) return true;
	view.dispatch({ changes });
	return true;
}

/** Shift-Tab on a list item moves it (and its children) out one level. */
export function outdentListItem(view: CommandView): boolean {
	const state = view.state;
	const lines = listLines(state);
	if (!lines) return false;
	const changes: ChangeSpec[] = [];
	const moved = new Set<number>();
	for (const number of lines) {
		if (moved.has(number)) continue;
		const line = state.doc.line(number);
		const own = indentWidth(line.text);
		if (own.length === 0) continue;
		const parent = previousItem(state, number, visualWidth(own) - 1);
		const target = parent ? indentWidth(parent.text) : '';
		const last = itemBlock(state, number);
		for (let child = number; child <= last; child += 1) {
			moved.add(child);
			const childLine = state.doc.line(child);
			if (childLine.text.trim() === '') continue;
			const childIndent = indentWidth(childLine.text);
			changes.push({
				from: childLine.from,
				to: childLine.from + childIndent.length,
				insert: target + childIndent.slice(own.length),
			});
		}
	}
	view.dispatch({ changes });
	return true;
}

export const formattingCommands = {
	bold: (view: CommandView) => toggleInline(view, 'bold'),
	italic: (view: CommandView) => toggleInline(view, 'italic'),
	underline: (view: CommandView) => togglePair(view, '<u>', '</u>'),
	strikethrough: (view: CommandView) => toggleInline(view, 'strikethrough'),
	code: (view: CommandView) => toggleInline(view, 'code'),
	bulletList: (view: CommandView) => toggleLines(view, 'bulletList'),
	numberedList: (view: CommandView) => toggleLines(view, 'numberedList'),
	checkList: (view: CommandView) => toggleLines(view, 'checkList'),
	blockQuote: (view: CommandView) => toggleLines(view, 'blockQuote'),
	link: (view: CommandView) => insertLink(view),
	heading: (level: 0 | 1 | 2 | 3) => (view: CommandView) =>
		setHeading(view, level),
};

const run =
	(command: (view: CommandView) => void) =>
	(view: CommandView): boolean => {
		if (view.state.readOnly) return false;
		command(view);
		return true;
	};

/** Obsidian's formatting shortcuts, plus list indentation on Tab. */
export const formattingKeymap: readonly KeyBinding[] = [
	{ key: 'Mod-b', run: run(formattingCommands.bold) },
	{ key: 'Mod-i', run: run(formattingCommands.italic) },
	{ key: 'Mod-u', run: run(formattingCommands.underline) },
	{ key: 'Mod-k', run: run(formattingCommands.link) },
	{ key: 'Mod-Shift-x', run: run(formattingCommands.strikethrough) },
	{ key: 'Mod-Enter', run: (view) => !view.state.readOnly && toggleTask(view) },
	{
		key: 'Tab',
		run: (view) => !view.state.readOnly && indentListItem(view),
		shift: (view) => !view.state.readOnly && outdentListItem(view),
	},
];

/**
 * The formats in effect at the main selection, for the toolbar's pressed
 * state.
 */
export function activeFormats(state: EditorState): MarkdownFormat[] {
	const formats = new Set<MarkdownFormat>();
	const range = state.selection.main;
	const tree = syntaxTree(state);
	for (
		let node: ReturnType<typeof tree.resolveInner> | null = tree.resolveInner(
			range.from,
			1,
		);
		node;
		node = node.parent
	) {
		if (node.to < range.to) continue;
		if (node.name === 'StrongEmphasis') formats.add('bold');
		else if (node.name === 'Emphasis') formats.add('italic');
		else if (node.name === 'Strikethrough') formats.add('strikethrough');
		else if (node.name === 'InlineCode') formats.add('code');
		else if (node.name === 'Link') formats.add('link');
	}
	for (const char of ['*', '_']) {
		const { from, to } = normalizeInline(state, range, char);
		const before = runBefore(state, from, char);
		const after = runAfter(state, to, char);
		if (before >= 2 && after >= 2) formats.add('bold');
		if (before % 2 === 1 && after % 2 === 1) formats.add('italic');
	}
	if (
		state.sliceDoc(range.from - 3, range.from) === '<u>' &&
		state.sliceDoc(range.to, range.to + 4) === '</u>'
	)
		formats.add('underline');
	const kind = lineShape(state.doc.lineAt(range.head).text).kind;
	if (kind) formats.add(kind);
	return [...formats];
}
