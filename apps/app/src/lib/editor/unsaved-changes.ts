import { toast } from 'svelte-sonner';
import {
	blockedDrafts,
	describeBlockedDrafts,
	discardPendingDrafts,
	flushPendingDrafts,
} from './pending-drafts.svelte';
import UnsavedChangesToast from './unsaved-changes-toast.svelte';

const TOAST_ID = 'unsaved-changes';

/**
 * Save every open editor before leaving it. Resolves true when everything
 * saved and the caller can go ahead. Otherwise it shows why, with a way to
 * try again, keep editing, or discard, and calls `proceed` itself if the
 * user gets past the problem.
 */
export async function saveBeforeLeaving(
	proceed: () => void | Promise<void>,
): Promise<boolean> {
	if (await flushPendingDrafts()) {
		toast.dismiss(TOAST_ID);
		return true;
	}
	explain(proceed);
	return false;
}

function explain(proceed: () => void | Promise<void>) {
	const drafts = blockedDrafts();
	const message = describeBlockedDrafts(drafts);
	toast.custom(UnsavedChangesToast, {
		id: TOAST_ID,
		duration: Number.POSITIVE_INFINITY,
		unstyled: true,
		componentProps: {
			...message,
			onretry: () => {
				void saveBeforeLeaving(proceed).then((saved) => {
					if (saved) void proceed();
				});
			},
			onkeep: () => toast.dismiss(TOAST_ID),
			ondiscard: () => {
				toast.dismiss(TOAST_ID);
				discardPendingDrafts();
				void proceed();
			},
		},
	});
}
