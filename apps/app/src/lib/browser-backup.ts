import type { BrowserWorkspaceSnapshot } from '@noura/browser-workspace';
import {
	BROWSER_EXPORT_SNAPSHOT_LIMITS,
	isExcludedFromSnapshot,
	validateRelativePath,
} from '@noura/browser-storage';

// UI memory budget, deliberately lower than the storage service's limits.
export const BACKUP_LIMITS = {
	maxFileBytes: 96 * 1024 * 1024,
	...BROWSER_EXPORT_SNAPSHOT_LIMITS,
} as const;
const FORMAT = 'noura.browser-backup';
function invalid(): never {
	throw new Error(
		'Invalid or unsupported Noura JSON backup. Check its format, paths and size limits.',
	);
}
function record(
	value: unknown,
	keys: string[],
): value is Record<string, unknown> {
	return (
		!!value &&
		typeof value === 'object' &&
		!Array.isArray(value) &&
		Object.keys(value).length === keys.length &&
		keys.every((key) => Object.hasOwn(value, key))
	);
}
function paths(entries: readonly { path: unknown }[]) {
	if (!entries.length || entries.length > BACKUP_LIMITS.maxEntries) invalid();
	const seen = new Set<string>();
	for (const entry of entries) {
		if (typeof entry.path !== 'string' || entry.path.length > 4096) invalid();
		try {
			validateRelativePath(entry.path);
		} catch {
			invalid();
		}
		// Reject lone UTF-16 surrogates rather than silently replacing path bytes.
		if (
			new TextDecoder().decode(new TextEncoder().encode(entry.path)) !==
			entry.path
		)
			invalid();
		if (isExcludedFromSnapshot(entry.path) || seen.has(entry.path)) invalid();
		seen.add(entry.path);
	}
	for (const path of seen) {
		const parts = path.split('/');
		parts.pop();
		while (parts.length) {
			if (seen.has(parts.join('/'))) invalid();
			parts.pop();
		}
	}
	if (!seen.has('.noura/workspace.yaml')) invalid();
}
function checkSize(size: number, total: number) {
	if (
		size > BACKUP_LIMITS.maxEntryBytes ||
		total > BACKUP_LIMITS.maxTotalBytes
	) {
		throw new Error(
			'Backup exceeds the 32 MiB per-file or 64 MiB total decoded limit.',
		);
	}
}
export function encodeBackup(snapshot: BrowserWorkspaceSnapshot): Blob {
	if (
		!snapshot ||
		snapshot.format !== 'noura.workspace-snapshot' ||
		snapshot.version !== 1 ||
		typeof snapshot.workspaceId !== 'string' ||
		snapshot.workspaceId.length === 0 ||
		snapshot.workspaceId.length > 128 ||
		!Array.isArray(snapshot.entries)
	)
		invalid();
	for (const entry of snapshot.entries)
		if (!entry || !(entry.bytes instanceof Uint8Array)) invalid();
	paths(snapshot.entries);
	let total = 0;
	for (const entry of snapshot.entries)
		checkSize(entry.bytes.length, (total += entry.bytes.length));
	const entries = snapshot.entries.map(({ path, bytes }) => {
		const chunks: string[] = [];
		for (let i = 0; i < bytes.length; i += 8192)
			chunks.push(String.fromCharCode(...bytes.subarray(i, i + 8192)));
		return { path, base64: btoa(chunks.join('')) };
	});
	const blob = new Blob(
		[
			JSON.stringify({
				format: FORMAT,
				version: 1,
				workspaceId: snapshot.workspaceId,
				entries,
			}),
		],
		{ type: 'application/json' },
	);
	if (blob.size > BACKUP_LIMITS.maxFileBytes) invalid();
	return blob;
}
export async function decodeBackup(
	file: Blob,
): Promise<BrowserWorkspaceSnapshot> {
	// Check before reading, parsing JSON or allocating decoded byte arrays.
	if (!file.size || file.size > BACKUP_LIMITS.maxFileBytes)
		throw new Error(
			'Select a nonempty Noura JSON backup no larger than 96 MiB.',
		);
	let value: unknown;
	try {
		value = JSON.parse(
			new TextDecoder('utf-8', { fatal: true }).decode(
				await file.arrayBuffer(),
			),
		);
	} catch {
		return invalid();
	}
	if (
		!record(value, ['format', 'version', 'workspaceId', 'entries']) ||
		value.format !== FORMAT ||
		value.version !== 1 ||
		typeof value.workspaceId !== 'string' ||
		value.workspaceId.length > 128 ||
		!Array.isArray(value.entries) ||
		value.entries.length > BACKUP_LIMITS.maxEntries
	)
		invalid();
	let total = 0;
	// Validate every entry and aggregate decoded length BEFORE any atob/Uint8Array allocation.
	const entries = value.entries.map((entry: unknown) => {
		if (
			!record(entry, ['path', 'base64']) ||
			typeof entry.path !== 'string' ||
			typeof entry.base64 !== 'string'
		)
			invalid();
		const text = entry.base64;
		if (
			text.length % 4 ||
			text.length > 4 * Math.ceil(BACKUP_LIMITS.maxEntryBytes / 3)
		)
			invalid();
		const padding = text.endsWith('==') ? 2 : text.endsWith('=') ? 1 : 0;
		const size = (text.length / 4) * 3 - padding;
		checkSize(size, (total += size));
		// Linear scan avoids regular-expression stack blowups on large strings.
		const alphabet =
			'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
		for (let i = 0; i < text.length - padding; i++)
			if (!alphabet.includes(text[i]!)) invalid();
		if (
			padding &&
			alphabet.indexOf(text[text.length - padding - 1]!) &
				(padding === 2 ? 15 : 3)
		)
			invalid();
		return { path: entry.path, base64: text };
	});
	paths(entries);
	return {
		format: 'noura.workspace-snapshot',
		version: 1,
		workspaceId: value.workspaceId,
		entries: entries.map(({ path, base64 }) => ({
			path,
			bytes: Uint8Array.from(atob(base64), (char) => char.charCodeAt(0)),
		})),
	};
}

export const IDENTITY_CONFLICT =
	'A workspace with this identity already exists in this browser. Nothing was overwritten. Open the existing workspace, or restore this backup in a separate browser profile. Workspace IDs are never changed to bypass a conflict.';
export function assertBackupActionAllowed(dirty: boolean) {
	if (dirty)
		throw new Error(
			'Save or explicitly discard your unsaved draft before backup or import.',
		);
}
export function assertFreshIdentity(
	id: string,
	existing: { workspaceId: string }[],
) {
	if (existing.some((item) => item.workspaceId === id))
		throw new Error(IDENTITY_CONFLICT);
}
export function downloadBackup(
	blob: Blob,
	name: string,
	doc = document,
	urls = URL,
	defer = (callback: () => void) => setTimeout(callback, 60_000),
) {
	const filename = `noura-${name.replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 64) || 'workspace'}.noura-backup.json`;
	const url = urls.createObjectURL(blob);
	const anchor = doc.createElement('a');
	try {
		anchor.href = url;
		anchor.download = filename;
		anchor.hidden = true;
		doc.body.append(anchor);
		anchor.click();
	} finally {
		anchor.remove();
		// Let the browser consume the URL before revoking it, including on click failure.
		defer(() => urls.revokeObjectURL(url));
	}
}
