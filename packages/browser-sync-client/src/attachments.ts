/** Encrypts, uploads, and queues one attachment for a managed object. */

import {
	attachmentPath,
	BrowserSyncError,
	BrowserSyncErrorCode,
	encryptAttachmentToSink,
	MemoryAttachmentSink,
	randomIdentifier,
	uploadBlob,
	viewAttachmentSource,
	type AttachmentSource,
	type FileChangeBlob,
} from '@noura/browser-sync';
import type { BrowserSyncStorage } from '@noura/browser-sync-engine';
import { MAX_BROWSER_ATTACHMENT_BYTES } from './attachment-fetcher';
import { createReconcileEngine, resolveObjectOwner } from './reconcile';
import type { BrowserSyncReconcileInput } from './types';

/** Insert a short suffix before a file name's extension. */
function withUniqueSuffix(name: string, suffix: string): string {
	const dot = name.lastIndexOf('.');
	if (dot > 0) return `${name.slice(0, dot)}-${suffix}${name.slice(dot)}`;
	return `${name}-${suffix}`;
}

/** Choose an attachment path that is absent from the local replica. */
async function uniqueAttachmentPath(
	storage: BrowserSyncStorage,
	objectId: string,
	name: string,
): Promise<string> {
	const base = attachmentPath(objectId, name);
	if ((await storage.read(base)) === null) return base;
	for (let attempt = 0; attempt < 8; attempt += 1) {
		const candidate = attachmentPath(
			objectId,
			withUniqueSuffix(name, randomIdentifier().slice(0, 6)),
		);
		if ((await storage.read(candidate)) === null) return candidate;
	}
	throw new BrowserSyncError(
		BrowserSyncErrorCode.InvalidOperation,
		'Could not choose a unique attachment path',
	);
}

/** Wrap a ciphertext source so each read reports absolute upload progress. */
function progressAttachmentSource(
	source: AttachmentSource,
	onProgress: (uploadedBytes: number, totalBytes: number) => void,
): AttachmentSource {
	return {
		size: source.size,
		async read(offset, length) {
			const bytes = await source.read(offset, length);
			onProgress(offset + bytes.length, source.size);
			return bytes;
		},
	};
}

/** Inputs for {@link runBrowserSyncSendAttachment}. */
export interface BrowserSyncSendAttachmentInput extends BrowserSyncReconcileInput {
	/** Object that owns the attachment; the containing note's object. */
	objectId: string;
	/** User-supplied file name, reduced to one portable path component. */
	name: string;
	/** Complete plaintext bytes. */
	bytes: Uint8Array;
	/** Called as ciphertext reaches the server, in bounded upload steps. */
	onProgress?: (uploadedBytes: number, totalBytes: number) => void;
	/**
	 * Largest plaintext accepted, defaulting to
	 * {@link MAX_BROWSER_ATTACHMENT_BYTES}. Injectable so tests can exercise the
	 * bound without allocating a large buffer.
	 */
	maxBytes?: number;
}

/** Result of a successful {@link runBrowserSyncSendAttachment}. */
export interface BrowserSyncSendAttachmentOutcome {
	/** Workspace path the version-3 change writes. */
	path: string;
	/** Signed encrypted descriptor carried by the queued change. */
	blob: FileChangeBlob;
}

/**
 * Attach one file to a managed object without a network round trip of its own
 * beyond the blob upload.
 *
 * The attachment is encrypted with the owning object's key using the same `age`
 * v1 construction as native, uploaded through the bounded resumable path (which
 * only resolves once the server acknowledges completion), written to the local
 * replica at `attachments/<objectId>/<name>`, then sealed and enqueued as a
 * version-3 file change. No operation is enqueued if encryption, upload, or the
 * local write fails. The caller reconciles to push the queued operation.
 */
export async function runBrowserSyncSendAttachment(
	input: BrowserSyncSendAttachmentInput,
): Promise<BrowserSyncSendAttachmentOutcome> {
	if (!input.attachments) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.RequestFailed,
			'No sync transport is configured, so an attachment cannot be uploaded.',
		);
	}
	if (input.bytes.length > (input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobTooLarge,
			`Attachments are limited to ${input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES} bytes in the browser.`,
		);
	}
	// Validate the name and resolve the owner before any encryption or upload.
	const candidate = attachmentPath(input.objectId, input.name);
	const owner = resolveObjectOwner(input, {
		path: candidate,
		previousPath: null,
	});
	if (!owner || owner.objectId !== input.objectId) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'The containing object is not bound for sync, so the attachment cannot be attached.',
		);
	}
	const objectKey = input.objectKeys.get(owner.objectId);
	if (!objectKey) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.MissingKey,
			'No object key is available for the containing object.',
		);
	}
	const path = await uniqueAttachmentPath(
		input.storage,
		owner.objectId,
		input.name,
	);

	const sink = new MemoryAttachmentSink();
	const blob = await encryptAttachmentToSink(
		objectKey,
		viewAttachmentSource(input.bytes),
		sink,
	);
	const ciphertext = sink.toBytes();
	await uploadBlob({
		origin: input.attachments.origin,
		token: input.attachments.token,
		fetch: input.attachments.fetch,
		workspaceId: input.workspaceId,
		objectId: owner.objectId,
		epoch: owner.epoch,
		blob,
		source:
			input.onProgress === undefined
				? viewAttachmentSource(ciphertext)
				: progressAttachmentSource(
						viewAttachmentSource(ciphertext),
						input.onProgress,
					),
	});

	// The blob is complete. Write the canonical bytes locally so the replica is
	// consistent, then enqueue the change. A failed seal must not leave a local
	// file that a later snapshot would seal as inline content.
	await input.storage.write({
		path,
		bytes: input.bytes,
		expectedRevision: null,
	});
	const engine = createReconcileEngine(input);
	try {
		await engine.enqueueFileChange({
			path,
			previousPath: null,
			baseRevision: null,
			content: null,
			blob,
		});
	} catch (error) {
		await input.storage.delete({ path }).catch(() => {});
		throw error;
	}
	return { path, blob };
}
