import { EditorSelection } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';

export interface FormattingCommand {
	(view: EditorView): void;
}

function toggleWrap(view: EditorView, marker: string): void {
	toggleWrapPair(view, marker, marker);
}

function toggleWrapPair(
	view: EditorView,
	opening: string,
	closing: string,
): void {
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	const ranges = view.state.selection.ranges;
	const allWrapped =
		ranges.length > 0 &&
		ranges.every((range) => {
			const text = view.state.sliceDoc(range.from, range.to);
			return (
				text.startsWith(opening) &&
				text.endsWith(closing) &&
				text.length >= opening.length + closing.length
			);
		});
	let tail = ranges[ranges.length - 1]?.to ?? 0;
	for (const range of ranges) {
		const text = view.state.sliceDoc(range.from, range.to);
		if (allWrapped) {
			const inner = text.slice(
				opening.length,
				Math.max(opening.length, text.length - closing.length),
			);
			changes.push({ from: range.from, to: range.to, insert: inner });
		} else if (text.length === 0) {
			changes.push({
				from: range.from,
				to: range.to,
				insert: opening + closing,
			});
		} else {
			changes.push({
				from: range.from,
				to: range.to,
				insert: opening + text + closing,
			});
		}
		tail = range.to + opening.length + closing.length;
	}
	view.dispatch({
		changes,
		selection: allWrapped ? undefined : EditorSelection.cursor(tail),
	});
}

function setLineHints(view: EditorView, prefix: string, exact?: string) {
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	const seen = new Set<number>();
	const toggles: Array<{ line: number; add: boolean }> = [];
	for (const range of view.state.selection.ranges) {
		const first = view.state.doc.lineAt(range.from).number;
		const last = view.state.doc.lineAt(range.to).number;
		for (let line = first; line <= last; line += 1) {
			if (seen.has(line)) continue;
			seen.add(line);
			const text = view.state.doc.line(line).text;
			if (exact !== undefined) {
				toggles.push({ line, add: !text.startsWith(exact) });
			} else {
				toggles.push({ line, add: !text.startsWith(prefix) });
			}
		}
	}
	for (const { line, add } of toggles) {
		const lineInfo = view.state.doc.line(line);
		if (add) {
			changes.push({ from: lineInfo.from, to: lineInfo.from, insert: prefix });
		} else {
			for (const candidate of [exact, prefix]) {
				if (candidate && lineInfo.text.startsWith(candidate)) {
					changes.push({
						from: lineInfo.from,
						to: lineInfo.from + candidate.length,
						insert: '',
					});
					break;
				}
			}
		}
	}
	view.dispatch({ changes });
}

function setHeading(view: EditorView, level: 0 | 1 | 2 | 3) {
	const marker = level === 0 ? '' : '#'.repeat(level) + ' ';
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	for (const range of view.state.selection.ranges) {
		const lineInfo = view.state.doc.lineAt(range.from);
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

export function toggleCheckboxes(view: EditorView, lines: number[]): void {
	const changes: Array<{ from: number; to: number; insert: string }> = [];
	for (const lineNumber of lines) {
		const lineInfo = view.state.doc.line(lineNumber);
		const empty = lineInfo.text.match(/^(\s*- )\[ \]( .*)$/);
		const checked = lineInfo.text.match(/^(\s*- )\[[xX]\]( .*)$/);
		if (empty) {
			changes.push({
				from: lineInfo.from + (empty[1]?.length ?? 0),
				to: lineInfo.from + (empty[1]?.length ?? 0) + 3,
				insert: '[x]',
			});
		} else if (checked) {
			changes.push({
				from: lineInfo.from + (checked[1]?.length ?? 0),
				to: lineInfo.from + (checked[1]?.length ?? 0) + 3,
				insert: '[ ]',
			});
		}
	}
	view.dispatch({ changes });
}

export const formattingCommands = {
	bold: (view: EditorView) => toggleWrap(view, '**'),
	italic: (view: EditorView) => toggleWrap(view, '*'),
	underline: (view: EditorView) => toggleWrapPair(view, '<u>', '</u>'),
	strikethrough: (view: EditorView) => toggleWrap(view, '~~'),
	code: (view: EditorView) => toggleWrap(view, '`'),
	bulletList: (view: EditorView) => setLineHints(view, '- '),
	numberedList: (view: EditorView) => setLineHints(view, '1. '),
	checkList: (view: EditorView) => setLineHints(view, '- [ ] ', '- [ ] '),
	blockQuote: (view: EditorView) => setLineHints(view, '> '),
	inlineCode: (view: EditorView) => toggleWrap(view, '`'),
	link: (view: EditorView) => {
		const range = view.state.selection.main;
		const text = view.state.sliceDoc(range.from, range.to);
		view.dispatch({
			changes: {
				from: range.from,
				to: range.to,
				insert: '[' + text + '](https://)',
			},
			selection: EditorSelection.cursor(range.from + text.length + 3),
		});
	},
	heading: (level: 0 | 1 | 2 | 3) => (view: EditorView) =>
		setHeading(view, level),
};
