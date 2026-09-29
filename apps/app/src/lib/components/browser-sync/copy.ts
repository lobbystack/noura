import type { BrowserSyncStatus, SyncNowOutcome } from '$lib/browser-sync';

export const statusLabels: Record<BrowserSyncStatus, string> = {
	unavailable: 'Not supported',
	locked: 'Locked',
	unlocked: 'Not set up',
	enrolled: 'Ready',
	error: 'Needs attention',
};

/** Pick the singular or plural form for a count, e.g. `1 file`, `2 files`. */
export function count(value: number, singular: string, plural: string) {
	return `${value} ${value === 1 ? singular : plural}`;
}

/** Text for an error from a controller result or a thrown value. */
export function errorText(cause: unknown, fallback: string): string {
	return cause instanceof Error && cause.message ? cause.message : fallback;
}

export function syncOutcomeText(outcome: SyncNowOutcome): string {
	if (outcome.status !== 'synced') return outcome.message;
	const parts = [`${outcome.pushed} sent`, `${outcome.applied} received`];
	if (outcome.conflicts > 0)
		parts.push(count(outcome.conflicts, 'conflict', 'conflicts'));
	return `Synced: ${parts.join(', ')}.`;
}

const conflictReasons: Record<string, string> = {
	revision_mismatch: 'Changed here and on another device.',
	missing_expected_file: 'Deleted here and changed on another device.',
	unexpected_file: 'A file with this name already exists here.',
	occupied_destination: 'Another file already uses the new name.',
	missing_move_source: 'Moved on another device, but missing here.',
	invalid_move: "Moved on another device in a way noura can't apply.",
	storage_rejected: "This browser couldn't save the other version.",
};

export function conflictReasonText(reason: string): string {
	return conflictReasons[reason] ?? 'Changed in two places.';
}
