import { ChangeSet, Text } from '@codemirror/state';
import { diff } from '@codemirror/merge';

/**
 * Bounded diffing: large, very different inputs fall back to a coarser (still
 * correct) change set instead of running in quadratic time on the UI thread.
 */
const DIFF_CONFIG = { scanLimit: 5_000, timeout: 100 };

/**
 * The minimal change set that turns `from` into `to`. Positions are string
 * indices, which match CodeMirror positions because every editor in this
 * package uses `\n` as its only line separator.
 */
export function changesBetween(from: string, to: string): ChangeSet {
	if (from === to) return ChangeSet.empty(from.length);
	return ChangeSet.of(
		diff(from, to, DIFF_CONFIG).map((change) => ({
			from: change.fromA,
			to: change.toA,
			insert: to.slice(change.fromB, change.toB),
		})),
		from.length,
	);
}

/**
 * Carry the changes that turned `base` into `target` over the local edits
 * that turned `base` into `local`. The result applies to `local` and never
 * drops a local insertion: text the user typed survives even when the other
 * side rewrote the surrounding range.
 */
export function rebaseChanges(
	base: string,
	local: string,
	target: string,
): ChangeSet {
	const external = changesBetween(base, target);
	if (base === local) return external;
	// At a shared position the other side goes first, so text typed at the
	// caret stays right before the caret.
	return external.map(changesBetween(base, local), true);
}

/** String form of {@link rebaseChanges}, for callers without an editor. */
export function rebaseText(
	base: string,
	local: string,
	target: string,
): string {
	if (base === local) return target;
	if (base === target) return local;
	return rebaseChanges(base, local, target).apply(textOf(local)).toString();
}

/**
 * CodeMirror text split on `\n` only, so a lone `\r` stays an ordinary
 * character, exactly as in the editors.
 */
export function textOf(value: string): Text {
	return Text.of(value.split('\n'));
}
