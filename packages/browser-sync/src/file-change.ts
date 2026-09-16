/**
 * Canonical file-change payload encoding and decoding.
 *
 * An encrypted operation carries one UTF-8 JSON file change. Version 1 is the
 * base record, version 2 adds `acceptedRevisions` for reviewed resolutions, and
 * version 3 carries an encrypted attachment `blob`. This module is the
 * TypeScript mirror of the Rust serializer in `crates/local-core`: the field
 * order, the absence of whitespace, and the JSON string escaping match
 * `serde_json`, and `docs/workspace-format/fixtures/sync-v1.json` is the shared
 * conformance fixture.
 *
 * `content` is canonical standard base64 of the complete file bytes, or `null`.
 * Parsing and validation are delegated to `syncFileChangeSchema` from
 * `@noura/workspace-schema`; this module never redefines the format semantics.
 */

import { syncFileChangeSchema } from '@noura/workspace-schema';
import { decodeUtf8, encodeUtf8, type Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

/** Version 3 encrypted-attachment descriptor. */
export interface FileChangeBlob {
	/** Lowercase SHA-256 digest of the entire ciphertext. */
	id: string;
	/** Ciphertext size in bytes. */
	size: number;
	/** Plaintext size in bytes. */
	plaintextSize: number;
	/** Lowercase BLAKE3 digest of the canonical plaintext bytes. */
	revision: string;
}

interface FileChangeCommon {
	/** Portable relative workspace path. */
	path: string;
	/** Move source, or `null`. */
	previousPath: string | null;
	/** Revision the change is based on, or `null` when the path must be absent. */
	baseRevision: string | null;
	/** Canonical standard base64 file bytes, or `null` for a deletion. */
	content: string | null;
}

/** Version 1 file change: create, update, or delete. */
export interface FileChangeV1 extends FileChangeCommon {
	version: 1;
}

/** Version 2 file change: a reviewed resolution with accepted revisions. */
export interface FileChangeV2 extends FileChangeCommon {
	version: 2;
	/** One or two distinct accepted revisions, including `null`. */
	acceptedRevisions: (string | null)[];
}

/** Version 3 file change: an encrypted attachment descriptor. */
export interface FileChangeV3 extends FileChangeCommon {
	version: 3;
	/** Encrypted attachment descriptor. Never absent. */
	blob: FileChangeBlob;
}

/** A decoded, canonical file change. */
export type FileChange = FileChangeV1 | FileChangeV2 | FileChangeV3;

/** Version 1 input. `previousPath`, `baseRevision`, and `content` default to `null`. */
export interface FileChangeInputV1 {
	version: 1;
	path: string;
	previousPath?: string | null;
	baseRevision?: string | null;
	content?: string | null;
}

/** Version 2 input. `previousPath`, `baseRevision`, and `content` default to `null`. */
export interface FileChangeInputV2 {
	version: 2;
	path: string;
	previousPath?: string | null;
	baseRevision?: string | null;
	content?: string | null;
	acceptedRevisions: (string | null)[];
}

/** Version 3 input. `previousPath`, `baseRevision`, and `content` default to `null`. */
export interface FileChangeInputV3 {
	version: 3;
	path: string;
	previousPath?: string | null;
	baseRevision?: string | null;
	content?: string | null;
	blob: FileChangeBlob;
}

/** Input accepted by {@link encodeFileChange}. */
export type FileChangeInput =
	FileChangeInputV1 | FileChangeInputV2 | FileChangeInputV3;

/**
 * Encode a file change as canonical UTF-8 JSON bytes.
 *
 * The change is validated and normalized with `syncFileChangeSchema`, then
 * serialized with no whitespace in the fixture's field order:
 * `version,path,previousPath,baseRevision,content[,acceptedRevisions][,blob]`.
 * The escaping matches `serde_json`; path strings that need JSON escapes are
 * already rejected by the schema.
 */
export function encodeFileChange(input: FileChangeInput): Bytes {
	const parsed = syncFileChangeSchema.safeParse(input);
	if (!parsed.success) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidFileChange,
			'file change did not match the schema',
		);
	}
	const change = parsed.data;
	const canonical: Record<string, unknown> = {
		version: change.version,
		path: change.path,
		previousPath: change.previousPath,
		baseRevision: change.baseRevision,
		content: change.content,
	};
	if (change.version === 2) {
		canonical.acceptedRevisions = change.acceptedRevisions;
	}
	if (change.version === 3) {
		canonical.blob = change.blob;
	}
	return encodeUtf8(JSON.stringify(canonical));
}

/**
 * Decode strict UTF-8 JSON file-change bytes and validate them.
 *
 * Bytes that are not valid UTF-8, not valid JSON, or do not satisfy
 * `syncFileChangeSchema` are rejected with a
 * {@link BrowserSyncErrorCode.InvalidFileChange} error.
 */
export function decodeFileChange(bytes: Uint8Array): FileChange {
	let text: string;
	try {
		text = decodeUtf8(bytes);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidFileChange,
			'file change was not valid UTF-8',
			{ cause },
		);
	}
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidFileChange,
			'file change was not valid JSON',
			{ cause },
		);
	}
	const parsed = syncFileChangeSchema.safeParse(value);
	if (!parsed.success) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidFileChange,
			'file change did not match the schema',
		);
	}
	return parsed.data as FileChange;
}
