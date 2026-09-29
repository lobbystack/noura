/**
 * Tracks editors with unsaved durable work. Note routing, route navigation,
 * and shutdown flush these before proceeding.
 */
export interface PendingDraftDetails {
	/** The file name, shown when saving fails. */
	label?: () => string;
	/** Why the last save failed, in plain words, if known. */
	problem?: () => string | null;
	/** Drop the unsaved changes so the editor can close. */
	discard?: () => void;
}

interface PendingDraft extends PendingDraftDetails {
	flush: () => Promise<boolean>;
	isPending: () => boolean;
}

const pending = new Map<string, PendingDraft>();

export function registerPendingDraft(
	id: string,
	flush: () => Promise<boolean>,
	isPending: () => boolean,
	details: PendingDraftDetails = {},
): () => void {
	const registration = { ...details, flush, isPending };
	pending.set(id, registration);
	return () => {
		if (pending.get(id) === registration) pending.delete(id);
	};
}

export function hasPendingDrafts(): boolean {
	return [...pending.values()].some(({ isPending }) => isPending());
}

export async function flushPendingDrafts(): Promise<boolean> {
	let all = true;
	for (const draft of [...pending.values()]) {
		try {
			if (draft.isPending() && !(await draft.flush())) all = false;
		} catch {
			all = false;
		}
	}
	return all;
}

export interface BlockedDraft {
	label: string | null;
	problem: string | null;
	canDiscard: boolean;
}

/** Editors whose changes are still unsaved after a flush. */
export function blockedDrafts(): BlockedDraft[] {
	return [...pending.values()]
		.filter((draft) => draft.isPending())
		.map((draft) => ({
			label: draft.label?.() ?? null,
			problem: draft.problem?.() ?? null,
			canDiscard: draft.discard !== undefined,
		}));
}

/** Drop every unsaved change that can be dropped. */
export function discardPendingDrafts() {
	for (const draft of [...pending.values()]) {
		if (draft.isPending()) draft.discard?.();
	}
}

/** The words shown when unsaved changes block leaving an editor. */
export function describeBlockedDrafts(drafts: BlockedDraft[]) {
	const labels = drafts.flatMap((draft) => (draft.label ? [draft.label] : []));
	const title =
		labels.length === 1
			? `Couldn’t save “${labels[0]}”`
			: 'Couldn’t save your changes';
	const problem = drafts.find((draft) => draft.problem)?.problem;
	const description = problem
		? `${problem} Your changes are still in the editor.`
		: 'Your changes are still in the editor.';
	return {
		title,
		description,
		canDiscard: drafts.length > 0 && drafts.every((draft) => draft.canDiscard),
	};
}
