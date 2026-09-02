import { describe, expect, test } from 'bun:test';
import { EditorState, type TransactionSpec } from '@codemirror/state';
import * as Y from 'yjs';
import { applyCanonicalText, markdownBlockStyle } from './factory';

describe('canonical editor updates', () => {
	test('replace the document and clear tracked local undo history', () => {
		let state = EditorState.create({ doc: 'local draft' });
		const document = new Y.Doc();
		const ytext = document.getText('markdown');
		const origin = {};
		const undoManager = new Y.UndoManager(ytext, {
			trackedOrigins: new Set([origin]),
		});
		document.transact(() => ytext.insert(0, 'local draft'), origin);
		const view = {
			get state() {
				return state;
			},
			dispatch(spec: TransactionSpec) {
				state = state.update(spec).state;
			},
		};

		applyCanonicalText(view, 'canonical file', undoManager);

		expect(state.doc.toString()).toBe('canonical file');
		expect(undoManager.undoStack).toHaveLength(0);
		document.destroy();
	});
});

describe('markdownBlockStyle', () => {
	test('reports the heading level at the selection line', () => {
		expect(markdownBlockStyle('plain text')).toBe('text');
		expect(markdownBlockStyle('# First')).toBe('heading1');
		expect(markdownBlockStyle('## Second')).toBe('heading2');
		expect(markdownBlockStyle('### Third')).toBe('heading3');
	});
});
