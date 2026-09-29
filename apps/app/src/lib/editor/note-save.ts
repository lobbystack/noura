import { isCoreError } from '@noura/workspace';
import { getNouraClient } from '$lib/state.svelte';
import type { Note } from '@noura/workspace';
import { reconcileNoteTitle } from './title-reconciliation';

export type NoteSaveResult =
	| { status: 'saved'; value: Note; merged: boolean }
	| { status: 'conflict'; draft: NoteDraft; current: Note };

export interface NoteDraft {
	title: string;
	body: string;
}

/**
 * Persist one note body with canonical-file reconciliation. Resolves with an
 * explicit conflict descriptor when overlapping edits require manual review;
 * durable write failures reject.
 */
export async function saveNoteWithReconciliation(
	note: Note,
	base: { revision: string; title: string; body: string },
	localDraft: NoteDraft,
): Promise<NoteSaveResult> {
	let currentDraft = localDraft;
	let currentBaseBody = base.body;
	let currentBaseTitle = base.title;
	let currentBaseRevision = base.revision;
	let merged = false;
	for (let attempt = 0; attempt < 3; attempt += 1) {
		const reconciliation = await getNouraClient().notes.reconcileDraft({
			id: note.id,
			baseRevision: currentBaseRevision,
			baseBody: currentBaseBody,
			localBody: currentDraft.body,
		});
		if (reconciliation.status === 'conflict') {
			return {
				status: 'conflict',
				draft: currentDraft,
				current: reconciliation.current as Note,
			};
		}
		// A changed revision can be a title or property edit; only an outside
		// change to the body means the text on screen must follow the file.
		if (
			reconciliation.status === 'merged' &&
			reconciliation.body !== currentDraft.body
		)
			merged = true;
		const canonical = reconciliation.current as Note;
		const title = reconcileNoteTitle(
			currentBaseTitle,
			currentDraft.title,
			canonical.title,
		);
		if (title.status === 'conflict') {
			return { status: 'conflict', draft: currentDraft, current: canonical };
		}
		currentDraft = { title: title.title, body: reconciliation.body };
		currentBaseBody = canonical.body;
		currentBaseTitle = canonical.title;
		currentBaseRevision = canonical.revision;
		try {
			const result = await getNouraClient().notes.update(note.id, {
				expectedRevision: currentBaseRevision,
				title: currentDraft.title,
				body: currentDraft.body,
			});
			return { status: 'saved', value: result.value as Note, merged };
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
