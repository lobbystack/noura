import { describe, expect, test } from 'bun:test';
import { EditorSelection, EditorState } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import {
	activeFormats,
	formattingCommands,
	indentListItem,
	outdentListItem,
	toggleCheckboxes,
	toggleTask,
	type CommandView,
} from './commands';
import { markdownSupport } from './factory';

function createState(docText: string) {
	return EditorState.create({ doc: docText });
}

function withSelection(testState: EditorState, from: number, to: number) {
	const selection = EditorSelection.single(from, to);
	return testState.update({ selection }).state;
}

// Command functions only need the pieces the commands touch: doc, selection,
// dispatch, and sliceDoc. A fake view keeps these tests DOM-free.
function fakeView(testState: EditorState) {
	let state = testState;
	return {
		get state() {
			return state;
		},
		sliceDoc: (from: number, to: number) => state.doc.sliceString(from, to),
		dispatch: (spec: { changes: any; selection?: any }) => {
			state = state.update({
				changes: spec.changes,
				selection: spec.selection,
			}).state;
		},
		focus: () => {},
		view: () => {},
	};
}

function commandResult(
	initial: string,
	from: number,
	to: number,
	command: (view: any) => void,
) {
	const view = fakeView(
		withSelection(createState(initial), from, to),
	) as unknown as EditorView;
	command(view);
	return view.state.doc.toString();
}

describe('formattingCommands', () => {
	test('bold wraps the selection', () => {
		expect(commandResult('hello', 0, 5, formattingCommands.bold)).toBe(
			'**hello**',
		);
		expect(commandResult('**hello**', 0, 9, formattingCommands.bold)).toBe(
			'hello',
		);
	});

	test('italic and strike wrap', () => {
		expect(commandResult('hi', 0, 2, formattingCommands.italic)).toBe('*hi*');
		expect(commandResult('hi', 0, 2, formattingCommands.strikethrough)).toBe(
			'~~hi~~',
		);
	});

	test('underline wraps with safe inline HTML', () => {
		expect(commandResult('hi', 0, 2, formattingCommands.underline)).toBe(
			'<u>hi</u>',
		);
		expect(commandResult('<u>hi</u>', 0, 9, formattingCommands.underline)).toBe(
			'hi',
		);
	});

	test('heading applies and replaces level', () => {
		const h1 = formattingCommands.heading(1);
		expect(commandResult('text', 0, 4, h1)).toBe('# text');
		const h2 = formattingCommands.heading(2);
		expect(commandResult('# existing', 0, 10, h2)).toBe('## existing');
		const paragraph = formattingCommands.heading(0);
		expect(commandResult('## existing', 0, 11, paragraph)).toBe('existing');
	});

	test('block quote and check list prefixes toggle', () => {
		expect(commandResult('line', 0, 4, formattingCommands.blockQuote)).toBe(
			'> line',
		);
		expect(commandResult('> line', 0, 6, formattingCommands.blockQuote)).toBe(
			'line',
		);
	});

	test('checkbox flips on demand', () => {
		const initial = '- [ ] one\n- [x] two';
		const view = fakeView(createState(initial)) as unknown as EditorView;
		toggleCheckboxes(view, [1, 2]);
		expect(view.state.doc.toString()).toBe('- [x] one\n- [ ] two');
	});

	test('checkbox flips on star, plus, ordered, and empty tasks', () => {
		const initial = '* [ ] a\n+ [x] b\n1. [ ] c\n- [ ]';
		const view = fakeView(createState(initial)) as unknown as EditorView;
		toggleCheckboxes(view, [1, 2, 3, 4]);
		expect(view.state.doc.toString()).toBe('* [x] a\n+ [ ] b\n1. [x] c\n- [x]');
	});
});

function run(
	initial: string,
	from: number,
	to: number,
	command: (view: CommandView) => unknown,
) {
	const view = fakeView(withSelection(markdownState(initial), from, to));
	command(view as unknown as CommandView);
	const range = view.state.selection.main;
	return {
		doc: view.state.doc.toString(),
		from: range.from,
		to: range.to,
	};
}

function markdownState(docText: string) {
	return EditorState.create({ doc: docText, extensions: [markdownSupport()] });
}

describe('inline formatting', () => {
	test('bold on an empty selection puts the caret between the markers', () => {
		expect(run('abc ', 4, 4, formattingCommands.bold)).toEqual({
			doc: 'abc ****',
			from: 6,
			to: 6,
		});
	});

	test('bold again on the empty pair removes it', () => {
		expect(run('abc ****', 6, 6, formattingCommands.bold)).toEqual({
			doc: 'abc ',
			from: 4,
			to: 4,
		});
	});

	test('bold keeps the wrapped text selected so it toggles back', () => {
		const wrapped = run('hello', 0, 5, formattingCommands.bold);
		expect(wrapped).toEqual({ doc: '**hello**', from: 2, to: 7 });
		expect(run(wrapped.doc, 2, 7, formattingCommands.bold).doc).toBe('hello');
	});

	test('toggles off when only the visible text is selected', () => {
		expect(run('a **bold** b', 4, 8, formattingCommands.bold).doc).toBe(
			'a bold b',
		);
		expect(run('a *it* b', 3, 5, formattingCommands.italic).doc).toBe('a it b');
		expect(run('a ~~x~~ b', 4, 5, formattingCommands.strikethrough).doc).toBe(
			'a x b',
		);
		expect(run('a `x` b', 3, 4, formattingCommands.code).doc).toBe('a x b');
		expect(run('a <u>x</u> b', 5, 6, formattingCommands.underline).doc).toBe(
			'a x b',
		);
	});

	test('italic on bold text adds italic instead of stripping bold', () => {
		expect(run('**bold**', 0, 8, formattingCommands.italic).doc).toBe(
			'***bold***',
		);
		expect(run('**bold**', 2, 6, formattingCommands.italic).doc).toBe(
			'***bold***',
		);
	});

	test('bold and italic toggle independently on bold italic text', () => {
		expect(run('***x***', 3, 4, formattingCommands.bold).doc).toBe('*x*');
		expect(run('***x***', 3, 4, formattingCommands.italic).doc).toBe('**x**');
	});

	test('link uses the selection as the label and puts the caret in the target', () => {
		expect(run('see docs', 4, 8, formattingCommands.link)).toEqual({
			doc: 'see [docs]()',
			from: 11,
			to: 11,
		});
		expect(run('https://a.b', 0, 11, formattingCommands.link)).toEqual({
			doc: '[](https://a.b)',
			from: 1,
			to: 1,
		});
		expect(run('', 0, 0, formattingCommands.link)).toEqual({
			doc: '[]()',
			from: 1,
			to: 1,
		});
	});
});

describe('line formatting', () => {
	test('numbered list numbers each selected line', () => {
		expect(run('a\nb\nc', 0, 5, formattingCommands.numberedList).doc).toBe(
			'1. a\n2. b\n3. c',
		);
	});

	test('switches between list kinds instead of stacking markers', () => {
		expect(run('1. a', 0, 4, formattingCommands.bulletList).doc).toBe('- a');
		expect(run('- a', 0, 3, formattingCommands.checkList).doc).toBe('- [ ] a');
		expect(run('- [ ] a', 0, 7, formattingCommands.checkList).doc).toBe('a');
	});

	test('Mod-Enter checks a task or turns a line into one', () => {
		expect(run('- [ ] a', 0, 0, toggleTask).doc).toBe('- [x] a');
		expect(run('- a', 0, 0, toggleTask).doc).toBe('- [ ] a');
		expect(run('a', 0, 0, toggleTask).doc).toBe('- [ ] a');
	});
});

describe('list indentation', () => {
	test('Tab nests an ordered item under the item above', () => {
		const result = run('1. parent\n2. child\n', 12, 12, indentListItem);
		expect(result.doc).toBe('1. parent\n   1. child\n');
		expect(markdownState(result.doc).doc.lines).toBe(3);
	});

	test('Tab nests a bullet with its children', () => {
		expect(run('- a\n- b\n  - c\n- d', 5, 5, indentListItem).doc).toBe(
			'- a\n  - b\n    - c\n- d',
		);
	});

	test('Shift-Tab moves an item back out', () => {
		expect(run('- a\n  - b\n', 7, 7, outdentListItem).doc).toBe('- a\n- b\n');
	});

	test('Tab outside a list keeps its ordinary meaning', () => {
		expect(indentListItem(fakeView(markdownState('text')) as never)).toBe(
			false,
		);
	});
});

describe('activeFormats', () => {
	test('reports the formats at the selection for the toolbar', () => {
		const state = withSelection(markdownState('a **b** c'), 4, 5);
		expect(activeFormats(state)).toContain('bold');
		const list = withSelection(markdownState('- [ ] task'), 8, 8);
		expect(activeFormats(list)).toContain('checkList');
		const plain = withSelection(markdownState('plain'), 2, 2);
		expect(activeFormats(plain)).toEqual([]);
	});
});
