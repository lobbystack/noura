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
	// Most updates touch a small region of a large file. Trim the shared
	// start and end first so only that region is diffed.
	let start = 0;
	const shortest = Math.min(from.length, to.length);
	while (start < shortest && from.charCodeAt(start) === to.charCodeAt(start))
		start += 1;
	let end = 0;
	while (
		end < shortest - start &&
		from.charCodeAt(from.length - 1 - end) ===
			to.charCodeAt(to.length - 1 - end)
	)
		end += 1;
	// Keep surrogate pairs whole.
	if (start > 0 && isHighSurrogate(from.charCodeAt(start - 1))) start -= 1;
	if (end > 0 && isLowSurrogate(from.charCodeAt(from.length - end))) end -= 1;
	const middleFrom = from.slice(start, from.length - end);
	const middleTo = to.slice(start, to.length - end);
	return ChangeSet.of(
		diff(middleFrom, middleTo, DIFF_CONFIG).map((change) => ({
			from: start + change.fromA,
			to: start + change.toA,
			insert: middleTo.slice(change.fromB, change.toB),
		})),
		from.length,
	);
}

function isHighSurrogate(code: number) {
	return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number) {
	return code >= 0xdc00 && code <= 0xdfff;
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
