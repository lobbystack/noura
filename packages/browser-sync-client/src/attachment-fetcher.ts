/** Downloads and decrypts version-3 attachment ciphertext for the engine. */

import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	decryptAttachment,
	downloadBlob,
	MemoryAttachmentSink,
	type FetchLike,
} from '@noura/browser-sync';
import type { AttachmentFetcher } from '@noura/browser-sync-engine';

/**
 * Largest attachment this browser will download and decrypt into memory before
 * handing the plaintext to the workspace storage boundary. This is well below
 * the protocol's 1 GiB limit because the browser has no streaming decrypt path
 * into OPFS yet; larger attachments are refused with a structured error.
 */
export const MAX_BROWSER_ATTACHMENT_BYTES = 64 * 1024 * 1024;

/**
 * Build the engine's attachment fetcher over the sync transport.
 *
 * A version-3 change is downloaded as bounded ciphertext ranges, verified
 * against its signed SHA-256 digest, decrypted with the object key recovered for
 * `change.objectId`, and returned only when the plaintext length matches the
 * signed descriptor. Any missing key, unavailable blob, oversized blob, or
 * failed authentication throws so the engine never writes an empty file.
 */
export function createBrowserAttachmentFetcher(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	objectKeys: ReadonlyMap<string, Uint8Array>;
	maxBytes?: number;
}): AttachmentFetcher {
	const maxBytes = input.maxBytes ?? MAX_BROWSER_ATTACHMENT_BYTES;
	return {
		async fetch(change) {
			const blob = change.blob;
			if (!blob) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidBlob,
					'An attachment fetch was requested for a change without a blob.',
				);
			}
			if (blob.size > maxBytes || blob.plaintextSize > maxBytes) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.BlobTooLarge,
					'This attachment is larger than the browser can decrypt in memory.',
				);
			}
			const objectKey = input.objectKeys.get(change.objectId);
			if (!objectKey) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.MissingKey,
					'No object key is available for the attachment object.',
				);
			}
			const sink = new MemoryAttachmentSink();
			await downloadBlob({
				origin: input.origin,
				token: input.token,
				fetch: input.fetch,
				workspaceId: input.workspaceId,
				objectId: change.objectId,
				epoch: change.epoch,
				blob,
				sink,
			});
			return decryptAttachment(objectKey, blob, sink.toBytes());
		},
	};
}
