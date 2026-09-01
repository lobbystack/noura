/**
 * Tracks editors with unsaved durable work. Note routing, route navigation,
 * and shutdown flush these before proceeding.
 */
interface PendingDraft {
	flush: () => Promise<boolean>;
	isPending: () => boolean;
}

const pending = new Map<string, PendingDraft>();

export function registerPendingDraft(
	id: string,
	flush: () => Promise<boolean>,
	isPending: () => boolean,
): () => void {
	const registration = { flush, isPending };
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
