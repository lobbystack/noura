import { describe, expect, test } from 'bun:test';
import { EditorSelection, EditorState } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import { formattingCommands, toggleCheckboxes } from './commands';

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
});
