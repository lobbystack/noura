import { describe, expect, test } from 'bun:test';
import { reconcileNoteTitle } from './title-reconciliation';

describe('reconcileNoteTitle', () => {
	test('keeps an in-app rename when the file title is unchanged', () => {
		expect(reconcileNoteTitle('Before', 'After', 'Before')).toEqual({
			status: 'resolved',
			title: 'After',
		});
	});

	test('adopts an external rename when the in-app title is unchanged', () => {
		expect(reconcileNoteTitle('Before', 'Before', 'Outside')).toEqual({
			status: 'resolved',
			title: 'Outside',
		});
	});

	test('accepts the same rename from both editors', () => {
		expect(reconcileNoteTitle('Before', 'Shared', 'Shared')).toEqual({
			status: 'resolved',
			title: 'Shared',
		});
	});

	test('requires review when both editors choose different titles', () => {
		expect(reconcileNoteTitle('Before', 'Mine', 'Outside')).toEqual({
			status: 'conflict',
		});
	});
});
