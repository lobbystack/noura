import { isCoreError } from '@noura/workspace';
import { getNouraClient } from '$lib/state.svelte';
import type { Note } from '@noura/workspace';

export type NoteSaveResult =
	| { status: 'saved'; value: Note }
	| { status: 'conflict'; draft: string; current: Note };

/**
 * Persist one note body with canonical-file reconciliation. Resolves with an
 * explicit conflict descriptor when overlapping edits require manual review;
 * durable write failures reject.
 */
export async function saveNoteWithReconciliation(
	note: Note,
	base: { revision: string; body: string },
	localBody: string,
): Promise<NoteSaveResult> {
	let currentBody = localBody;
	let currentBaseBody = base.body;
	let currentBaseRevision = base.revision;
	for (let attempt = 0; attempt < 3; attempt += 1) {
		const reconciliation = await getNouraClient().notes.reconcileDraft({
			id: note.id,
			baseRevision: currentBaseRevision,
			baseBody: currentBaseBody,
			localBody: currentBody,
		});
		if (reconciliation.status === 'conflict') {
			return {
				status: 'conflict',
				draft: currentBody,
				current: reconciliation.current as Note,
			};
		}
		currentBody = reconciliation.body;
		currentBaseBody = reconciliation.current.body;
		currentBaseRevision = reconciliation.current.revision;
		try {
			const result = await getNouraClient().notes.update(note.id, {
				expectedRevision: currentBaseRevision,
				body: currentBody,
			});
			return { status: 'saved', value: result.value as Note };
		} catch (error) {
			if (
				!isCoreError(error) ||
				error.code !== 'revision_conflict' ||
				attempt === 2
			)
				throw error;
		}
	}
	throw new Error('The note could not be reconciled automatically');
}
