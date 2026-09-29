/**
 * Durable store for one workspace's browser sync binding record.
 *
 * Hosts supply the durable implementation; {@link createMemoryBindingStore} is
 * for tests only.
 */

import {
	isBrowserSyncBindingRecord,
	type BrowserSyncBindingRecord,
	type BrowserSyncBoundObject,
} from './binding';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

/** Durable store for one workspace's binding record, outside canonical files. */
export interface BrowserSyncBindingStore {
	read(): Promise<BrowserSyncBindingRecord | null>;
	write(record: BrowserSyncBindingRecord): Promise<void>;
	remove(): Promise<boolean>;
}

function cloneBindingRecord(
	record: BrowserSyncBindingRecord,
): BrowserSyncBindingRecord {
	const objects: Record<string, BrowserSyncBoundObject> = {};
	for (const [objectId, bound] of Object.entries(record.objects)) {
		objects[objectId] = { ...bound, key: { ...bound.key } };
	}
	return {
		...record,
		objects,
		pinnedSigners: { ...record.pinnedSigners },
	};
}

/** In-memory binding store for tests. Not durable. */
export function createMemoryBindingStore(
	initial?: BrowserSyncBindingRecord,
): BrowserSyncBindingStore {
	let record = initial ? cloneBindingRecord(initial) : null;
	return {
		async read() {
			return record ? cloneBindingRecord(record) : null;
		},
		async write(next) {
			record = cloneBindingRecord(next);
		},
		async remove() {
			const had = record !== null;
			record = null;
			return had;
		},
	};
}

/**
 * Parse a stored binding record, rejecting invalid JSON or an unexpected shape
 * with a structured `InvalidBundle` error.
 */
export function parseBrowserSyncBindingRecord(
	text: string,
): BrowserSyncBindingRecord {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser sync binding was not valid JSON',
		);
	}
	if (!isBrowserSyncBindingRecord(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The stored browser sync binding had an unexpected shape',
		);
	}
	return value;
}
