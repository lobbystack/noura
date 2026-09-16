/**
 * File-change codec: the bridge between the replica engine's byte-oriented
 * file-change descriptor and sealed `EncryptedOperation` envelopes.
 *
 * `createFileChangeCodec` seals a descriptor with the workspace object key and
 * the device signing identity, and opens a sealed operation by verifying it
 * against a caller-pinned signer before decrypting and decoding the canonical
 * file-change payload. It never trusts the server or an unpinned signer.
 *
 * The codec performs no I/O and never logs key material. It composes
 * {@link sealOperation} / {@link openOperation} from `./operations` with the
 * canonical {@link encodeFileChange} / {@link decodeFileChange} format codec.
 * JavaScript cannot guarantee memory zeroization; this module clears references
 * it owns only.
 */

import type { EncryptedOperation } from '@noura/shared';
import {
	SECRET_LENGTH,
	decodeBase64,
	encodeBase64,
	type Bytes,
} from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import {
	decodeFileChange,
	encodeFileChange,
	type FileChange,
	type FileChangeBlob,
	type FileChangeInputV1,
	type FileChangeInputV3,
} from './file-change';
import type { DeviceIdentity } from './identity';
import { openOperation, sealOperation, validateOperation } from './operations';

/**
 * A runtime file change with raw file bytes.
 *
 * This is the shape the local replica engine seals and applies; content is
 * converted to and from canonical base64 only inside the canonical format
 * codec.
 */
export interface FileChangeDescriptor {
	/** Portable relative workspace path. */
	path: string;
	/** Move source, or `null`. */
	previousPath: string | null;
	/** Revision the change is based on, or `null` when the path must be absent. */
	baseRevision: string | null;
	/** Complete file bytes, or `null` for a deletion or a version-3 attachment. */
	content: Uint8Array | null;
	/**
	 * Version-3 encrypted attachment descriptor. When present, `content` must be
	 * `null` and the sealed payload is version 3; otherwise a version 1 payload
	 * is sealed.
	 */
	blob?: FileChangeBlob;
}

/** An opened file change with the envelope identity recovered by the codec. */
export interface OpenedFileChange extends FileChangeDescriptor {
	/** Workspace the operation was bound to. */
	workspaceId: string;
	/** Object whose key decrypted the operation. */
	objectId: string;
	/** Key epoch the operation was sealed under. */
	epoch: number;
	/** Payload version that was decoded. */
	version: 1 | 2 | 3;
	/** Accepted revisions, present when the payload is version 2. */
	acceptedRevisions?: (string | null)[];
	/** Attachment descriptor, present when the payload is version 3. */
	blob?: FileChangeBlob;
}

/** Input for {@link FileChangeCodec.sealFileChange}. */
export interface SealFileChangeInput {
	/** Workspace identifier bound into the operation AAD and signature. */
	workspaceId: string;
	/** Object whose key seals the change. */
	objectId: string;
	/** Positive safe-integer key epoch supplied by the caller. */
	epoch: number;
	/** Canonical nonnegative decimal access-policy revision. */
	policyRevision: string;
	/** File change to encode and encrypt. */
	change: FileChangeDescriptor;
}

/**
 * Encrypted file-change boundary consumed by the local replica engine.
 *
 * A host binds the envelope identity (workspace, object, epoch, policy
 * revision) for each seal; opening recovers that identity from the operation.
 * An untrusted signer is reported as `browser_sync_untrusted_signer`, which a
 * host that treats it as revocation may map to its own revoked code.
 */
export interface FileChangeCodec {
	/** Seal a file change into an encrypted operation. */
	sealFileChange(input: SealFileChangeInput): Promise<EncryptedOperation>;
	/** Verify and open an encrypted operation back to a file change. */
	openFileChange(operation: EncryptedOperation): Promise<OpenedFileChange>;
}

/** Resolve an object key by object id. */
export type ObjectKeyResolver = (objectId: string) => Uint8Array | undefined;

/** Resolve a pinned base64 Ed25519 signer public key by device id. */
export type PinnedSignerResolver = (deviceId: string) => string | undefined;

/** Options for {@link createFileChangeCodec}. */
export interface FileChangeCodecOptions {
	/** Unlocked device identity supplying the signing seed and device id. */
	identity: Pick<DeviceIdentity, 'signingSeed' | 'deviceId'>;
	/** Object keys by object id, or a resolver. */
	objectKeys: ReadonlyMap<string, Uint8Array> | ObjectKeyResolver;
	/** Pinned base64 Ed25519 signer public keys by device id, or a resolver. */
	pinnedSigners: ReadonlyMap<string, string> | PinnedSignerResolver;
}

function resolveObjectKey(
	objectKeys: FileChangeCodecOptions['objectKeys'],
	objectId: string,
): Bytes {
	const key =
		typeof objectKeys === 'function'
			? objectKeys(objectId)
			: objectKeys.get(objectId);
	if (key === undefined) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.MissingKey,
			'no object key was available for the operation object',
		);
	}
	return new Uint8Array(key);
}

function resolveSigner(
	pinnedSigners: FileChangeCodecOptions['pinnedSigners'],
	deviceId: string,
): string {
	const value =
		typeof pinnedSigners === 'function'
			? pinnedSigners(deviceId)
			: pinnedSigners.get(deviceId);
	if (value === undefined) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UntrustedSigner,
			'operation signer was not pinned',
		);
	}
	return value;
}

function resolveSignerKey(
	pinnedSigners: FileChangeCodecOptions['pinnedSigners'],
	deviceId: string,
): Bytes {
	const encoded = resolveSigner(pinnedSigners, deviceId);
	try {
		return decodeBase64(encoded, SECRET_LENGTH);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UntrustedSigner,
			'pinned signer key was malformed',
			{ cause },
		);
	}
}

function toOpenedFileChange(
	change: FileChange,
	operation: EncryptedOperation,
): OpenedFileChange {
	const base = {
		workspaceId: operation.workspaceId,
		objectId: operation.objectId,
		epoch: operation.epoch,
		version: change.version,
		path: change.path,
		previousPath: change.previousPath,
		baseRevision: change.baseRevision,
		content: change.content === null ? null : decodeBase64(change.content),
	};
	if (change.version === 2) {
		return { ...base, acceptedRevisions: change.acceptedRevisions };
	}
	if (change.version === 3) {
		return { ...base, blob: change.blob };
	}
	return base;
}

/**
 * Create a file-change codec over an unlocked identity, object keys, and pinned
 * signers.
 *
 * Sealing emits a version 3 payload when the descriptor carries a `blob`
 * (attachment) and a version 1 payload otherwise; opening decodes any supported
 * payload version. A missing object key is reported as
 * `browser_sync_missing_key` and an absent trust pin as
 * `browser_sync_untrusted_signer`.
 */
export function createFileChangeCodec(
	options: FileChangeCodecOptions,
): FileChangeCodec {
	return {
		async sealFileChange(input) {
			const canonical: FileChangeInputV1 | FileChangeInputV3 =
				input.change.blob === undefined
					? {
							version: 1,
							path: input.change.path,
							previousPath: input.change.previousPath,
							baseRevision: input.change.baseRevision,
							content:
								input.change.content === null
									? null
									: encodeBase64(input.change.content),
						}
					: {
							version: 3,
							path: input.change.path,
							previousPath: input.change.previousPath,
							baseRevision: input.change.baseRevision,
							content:
								input.change.content === null
									? null
									: encodeBase64(input.change.content),
							blob: input.change.blob,
						};
			const plaintext = encodeFileChange(canonical);
			return sealOperation({
				objectKey: resolveObjectKey(options.objectKeys, input.objectId),
				workspaceId: input.workspaceId,
				objectId: input.objectId,
				deviceId: options.identity.deviceId,
				epoch: input.epoch,
				policyRevision: input.policyRevision,
				plaintext,
				identity: options.identity,
			});
		},

		async openFileChange(operation) {
			const envelope = validateOperation(operation);
			const trustedSigningPublicKey = resolveSignerKey(
				options.pinnedSigners,
				envelope.deviceId,
			);
			const objectKey = resolveObjectKey(options.objectKeys, envelope.objectId);
			const plaintext = await openOperation({
				operation: envelope,
				objectKey,
				trustedSigningPublicKey,
			});
			return toOpenedFileChange(decodeFileChange(plaintext), envelope);
		},
	};
}
