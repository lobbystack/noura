/**
 * Encrypted attachment (sync payload version 3) cryptography and transport for
 * browser devices.
 *
 * A native client encrypts large files with `age`'s authenticated streaming
 * format. The recipient is an X25519 identity derived from the workspace object
 * key with BLAKE3 `derive_key` context `noura.sync.blob.x25519.v1`. This module
 * reproduces that format byte-for-byte in TypeScript: X25519 ECDH, HKDF-SHA256,
 * ChaCha20-Poly1305, the `age-encryption.org/v1` header with its HMAC, and the
 * 64 KiB STREAM payload. The ephemeral keys, file key, and payload nonce are
 * always freshly random, exactly as native does, so encryption is not
 * deterministic; a native client and this module decrypt each other's output.
 *
 * An attachment stays inside the signed, encrypted operation: only the
 * ciphertext SHA-256 digest, its size, the object id, and the key epoch reach the
 * server. The plaintext size, content revision, and path never leave the
 * operation. Uploads and downloads use the same resumable `tus`/range protocol as
 * native, in bounded 1 MiB steps, and every byte range is verified against the
 * signed SHA-256 digest before it is trusted.
 *
 * JavaScript cannot guarantee memory zeroization; this module clears references
 * it owns only and never logs key material or plaintext.
 */

import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { blake3 } from '@noble/hashes/blake3.js';
import { hkdf } from '@noble/hashes/hkdf.js';
import { hmac } from '@noble/hashes/hmac.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { bytesEqual, fixedBytes, isIdentifier, type Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import type { FileChangeBlob, FileChangeV3 } from './file-change';
import { ensureResponseOk, readJson, type FetchLike } from './http';

/** BLAKE3 `derive_key` context binding an object key to its blob identity. */
export const BLOB_KEY_CONTEXT = 'noura.sync.blob.x25519.v1';

/** Maximum accepted ciphertext size in bytes, matching native `MAX_BLOB_BYTES`. */
export const MAX_BLOB_BYTES = 1024 * 1024 * 1024;

/** Plaintext bytes per STREAM chunk, matching native age's 64 KiB chunk. */
export const BLOB_CHUNK_BYTES = 64 * 1024;

/** ChaCha20-Poly1305 authentication tag length in bytes. */
export const BLOB_TAG_BYTES = 16;

/** Encrypted STREAM chunk size (plaintext chunk plus its tag). */
export const BLOB_ENCRYPTED_CHUNK_BYTES = BLOB_CHUNK_BYTES + BLOB_TAG_BYTES;

/** Ciphertext bytes per upload PATCH and download range, matching the server. */
export const BLOB_UPLOAD_CHUNK_BYTES = 1024 * 1024;

/** Maximum header bytes read before the payload, matching the native budget. */
export const BLOB_HEADER_BUDGET_BYTES = 8192;

const AGE_MAGIC = 'age-encryption.org/v1\n';
const AGE_X25519_TAG = 'X25519';
const AGE_X25519_LABEL = new TextEncoder().encode(
	'age-encryption.org/v1/X25519',
);
const AGE_HEADER_LABEL = new TextEncoder().encode('header');
const AGE_PAYLOAD_LABEL = new TextEncoder().encode('payload');
const FILE_KEY_BYTES = 16;
const EPHEMERAL_PUBLIC_BYTES = 32;
const WRAPPED_FILE_KEY_BYTES = FILE_KEY_BYTES + BLOB_TAG_BYTES;
const X25519_PKCS8_PREFIX = [
	0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04,
	0x22, 0x04, 0x20,
];

/**
 * Random-access byte source for ciphertext or plaintext. `read` must return
 * exactly `length` bytes or throw; callers keep buffers bounded by requesting at
 * most {@link BLOB_UPLOAD_CHUNK_BYTES} at a time.
 */
export interface AttachmentSource {
	/** Total bytes available. */
	readonly size: number;
	/** Read exactly `length` bytes starting at `offset`. */
	read(offset: number, length: number): Promise<Uint8Array>;
}

/** Write-only destination for ciphertext or plaintext, addressed by offset. */
export interface AttachmentSink {
	/** Write `bytes` at `offset`. Calls are sequential and non-overlapping. */
	write(offset: number, bytes: Uint8Array): Promise<void>;
}

/** Location of one ciphertext blob on the sync service. */
export interface AttachmentLocation {
	/** Workspace the blob belongs to. */
	workspaceId: string;
	/** Object whose key epoch the blob is bound to. */
	objectId: string;
	/** Positive safe-integer key epoch. */
	epoch: number;
}

/** Result of encrypting attachment bytes. */
export interface EncryptedAttachment {
	/** Complete `age` ciphertext. */
	ciphertext: Bytes;
	/** Descriptor carried inside the signed version-3 file change. */
	blob: FileChangeBlob;
}

/** Inputs for {@link buildAttachmentFileChange}. */
export interface AttachmentFileChangeInput {
	/** Portable relative workspace path. */
	path: string;
	/** Move source, or `null`. */
	previousPath?: string | null;
	/** Revision the change is based on, or `null` when the path must be absent. */
	baseRevision?: string | null;
	/** Encrypted attachment descriptor. */
	blob: FileChangeBlob;
}

function hex(bytes: Uint8Array): string {
	let output = '';
	for (const byte of bytes) output += byte.toString(16).padStart(2, '0');
	return output;
}

function isDigest(value: unknown): value is string {
	return typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
}

/**
 * Validate an attachment descriptor exactly as native `EncryptedBlob::validate`
 * does: a nonempty bounded ciphertext, a plaintext smaller than the ciphertext,
 * and lowercase 64-character digests.
 */
export function validateAttachmentBlob(blob: FileChangeBlob): void {
	if (
		!blob ||
		typeof blob !== 'object' ||
		!isDigest(blob.id) ||
		!isDigest(blob.revision) ||
		!Number.isSafeInteger(blob.size) ||
		!Number.isSafeInteger(blob.plaintextSize) ||
		blob.size <= 0 ||
		blob.size > MAX_BLOB_BYTES ||
		blob.plaintextSize < 0 ||
		blob.plaintextSize >= blob.size
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment descriptor was malformed',
		);
	}
}

/**
 * Derive the 32-byte BLAKE3 `derive_key` seed that binds an object key to its
 * attachment identity. The native client encodes this same seed as an
 * `AGE-SECRET-KEY-` identity.
 */
export function deriveBlobSeed(objectKey: Uint8Array): Bytes {
	const key = fixedBytes(objectKey, 32, 'object key');
	return new Uint8Array(blake3(key, { context: AGE_BLOB_CONTEXT }));
}

const AGE_BLOB_CONTEXT = new TextEncoder().encode(BLOB_KEY_CONTEXT);

async function x25519(
	privateSeed: Uint8Array,
	publicKey: Uint8Array,
): Promise<Bytes> {
	const seed = fixedBytes(privateSeed, 32, 'X25519 seed');
	const pkcs8 = new Uint8Array(X25519_PKCS8_PREFIX.length + 32);
	pkcs8.set(X25519_PKCS8_PREFIX, 0);
	pkcs8.set(seed, X25519_PKCS8_PREFIX.length);
	try {
		const privateKey = await globalThis.crypto.subtle.importKey(
			'pkcs8',
			pkcs8,
			{ name: 'X25519' },
			false,
			['deriveBits'],
		);
		const publicCrypto = await globalThis.crypto.subtle.importKey(
			'raw',
			fixedBytes(publicKey, EPHEMERAL_PUBLIC_BYTES, 'X25519 public key'),
			{ name: 'X25519' },
			false,
			[],
		);
		const bits = await globalThis.crypto.subtle.deriveBits(
			{ name: 'X25519', public: publicCrypto },
			privateKey,
			256,
		);
		return new Uint8Array(bits);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment key agreement failed',
			{ cause },
		);
	}
}

async function publicFromSeed(seed: Uint8Array): Promise<Bytes> {
	const basePoint = new Uint8Array(32);
	basePoint[0] = 9;
	return x25519(seed, basePoint);
}

/** Derived attachment identity: the private seed and its X25519 public key. */
export interface BlobRecipient {
	/** 32-byte X25519 private seed derived from the object key. */
	seed: Bytes;
	/** 32-byte X25519 public key the seed addresses. */
	publicKey: Bytes;
}

/** Derive the attachment X25519 identity from an object key. */
export async function deriveBlobRecipient(
	objectKey: Uint8Array,
): Promise<BlobRecipient> {
	const seed = deriveBlobSeed(objectKey);
	return { seed, publicKey: await publicFromSeed(seed) };
}

function encodeBase64NoPad(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return globalThis.btoa(binary).replace(/=+$/u, '');
}

function decodeBase64NoPad(value: unknown, expectedLength?: number): Bytes {
	if (
		typeof value !== 'string' ||
		value.length > 1024 ||
		!/^[A-Za-z0-9+/]*$/u.test(value) ||
		value.length % 4 === 1
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header contained invalid base64',
		);
	}
	const padded = value + '='.repeat((4 - (value.length % 4)) % 4);
	let binary: string;
	try {
		binary = globalThis.atob(padded);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header contained invalid base64',
			{ cause },
		);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1) {
		bytes[index] = binary.charCodeAt(index);
	}
	if (encodeBase64NoPad(bytes) !== value) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header base64 was not canonical',
		);
	}
	if (expectedLength !== undefined && bytes.length !== expectedLength) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header field had the wrong length',
		);
	}
	return bytes;
}

function latin1(bytes: Uint8Array): string {
	let output = '';
	for (const byte of bytes) output += String.fromCharCode(byte);
	return output;
}

function streamNonce(counter: number, last: boolean): Bytes {
	const nonce = new Uint8Array(12);
	let value = counter;
	for (let index = 10; index >= 0; index -= 1) {
		nonce[index] = value & 0xff;
		value = Math.floor(value / 256);
	}
	nonce[11] = last ? 1 : 0;
	return nonce;
}

interface AgeStanza {
	tag: string;
	args: string[];
	body: Bytes;
}

/** Bytes wrapper over an in-memory buffer. */
export function bytesAttachmentSource(bytes: Uint8Array): AttachmentSource {
	const copy = bytes.slice();
	return {
		size: copy.length,
		async read(offset, length) {
			if (
				!Number.isSafeInteger(offset) ||
				!Number.isSafeInteger(length) ||
				offset < 0 ||
				length < 0 ||
				offset + length > copy.length
			) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidBlob,
					'attachment read was out of bounds',
				);
			}
			return copy.subarray(offset, offset + length);
		},
	};
}

/** In-memory {@link AttachmentSink}; callers must discard it if decrypt throws. */
export class MemoryAttachmentSink implements AttachmentSink {
	#parts: Uint8Array[] = [];
	#size = 0;

	async write(offset: number, bytes: Uint8Array): Promise<void> {
		if (offset !== this.#size) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidBlob,
				'attachment sink writes were not sequential',
			);
		}
		const copy = bytes.slice();
		this.#parts.push(copy);
		this.#size += copy.length;
	}

	/** Concatenate every written chunk. */
	toBytes(): Bytes {
		const output = new Uint8Array(this.#size);
		let offset = 0;
		for (const part of this.#parts) {
			output.set(part, offset);
			offset += part.length;
		}
		return output;
	}
}

/** BLAKE3 content revision of canonical plaintext bytes. */
export function attachmentRevision(plaintext: Uint8Array): string {
	return hex(blake3(plaintext));
}

/** Lowercase SHA-256 ciphertext digest. */
export function attachmentDigest(ciphertext: Uint8Array): string {
	return hex(sha256(ciphertext));
}

/**
 * Encrypt attachment bytes into `age` ciphertext, streaming through an injected
 * sink. Plaintext is read in 64 KiB chunks and never buffered as a whole.
 */
export async function encryptAttachmentToSink(
	objectKey: Uint8Array,
	source: AttachmentSource,
	sink: AttachmentSink,
): Promise<FileChangeBlob> {
	if (
		!Number.isSafeInteger(source.size) ||
		source.size < 0 ||
		source.size >= MAX_BLOB_BYTES
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobTooLarge,
			'attachment plaintext exceeded the protocol limit',
		);
	}
	const { seed, publicKey } = await deriveBlobRecipient(objectKey);
	const fileKey = randomFileKey();
	const ephemeralSeed = new Uint8Array(32);
	globalThis.crypto.getRandomValues(ephemeralSeed);
	const ephemeralPublic = await publicFromSeed(ephemeralSeed);
	const shared = await x25519(ephemeralSeed, publicKey);
	const salt = new Uint8Array(64);
	salt.set(ephemeralPublic, 0);
	salt.set(publicKey, 32);
	const wrappingKey = hkdf(sha256, shared, salt, AGE_X25519_LABEL, 32);
	const wrapped = chacha20poly1305(wrappingKey, new Uint8Array(12)).encrypt(
		fileKey,
	);
	if (wrapped.length !== WRAPPED_FILE_KEY_BYTES) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment file key wrap had the wrong length',
		);
	}
	const headerWithoutMac =
		AGE_MAGIC +
		`-> ${AGE_X25519_TAG} ${encodeBase64NoPad(ephemeralPublic)}\n` +
		`${encodeBase64NoPad(wrapped)}\n` +
		'---';
	const headerText =
		headerWithoutMac +
		` ${encodeBase64NoPad(
			hmac(
				sha256,
				hkdf(sha256, fileKey, undefined, AGE_HEADER_LABEL, 32),
				new TextEncoder().encode(headerWithoutMac),
			),
		)}\n`;
	const headerBytes = new TextEncoder().encode(headerText);
	const nonce = new Uint8Array(16);
	globalThis.crypto.getRandomValues(nonce);
	const payloadKey = hkdf(sha256, fileKey, nonce, AGE_PAYLOAD_LABEL, 32);

	const ciphertextHash = sha256.create();
	ciphertextHash.update(headerBytes);
	ciphertextHash.update(nonce);
	await sink.write(0, headerBytes);
	await sink.write(headerBytes.length, nonce);

	const plaintextHash = blake3.create();
	let ciphertextSize = headerBytes.length + nonce.length;
	let plaintextSize = 0;
	let offset = 0;
	let counter = 0;
	let remaining = source.size;
	while (remaining > BLOB_CHUNK_BYTES) {
		const chunk = await readExact(source, offset, BLOB_CHUNK_BYTES);
		plaintextHash.update(chunk);
		const encrypted = chacha20poly1305(
			payloadKey,
			streamNonce(counter, false),
		).encrypt(chunk);
		await sink.write(ciphertextSize, encrypted);
		ciphertextHash.update(encrypted);
		plaintextSize += chunk.length;
		ciphertextSize += encrypted.length;
		offset += chunk.length;
		remaining -= chunk.length;
		counter += 1;
	}
	const finalChunk =
		remaining === 0
			? new Uint8Array()
			: await readExact(source, offset, remaining);
	plaintextHash.update(finalChunk);
	const encryptedFinal = chacha20poly1305(
		payloadKey,
		streamNonce(counter, true),
	).encrypt(finalChunk);
	await sink.write(ciphertextSize, encryptedFinal);
	ciphertextHash.update(encryptedFinal);
	plaintextSize += finalChunk.length;
	ciphertextSize += encryptedFinal.length;

	const blob: FileChangeBlob = {
		id: hex(ciphertextHash.digest()),
		size: ciphertextSize,
		plaintextSize,
		revision: hex(plaintextHash.digest()),
	};
	validateAttachmentBlob(blob);
	return blob;
}

function randomFileKey(): Bytes {
	const fileKey = new Uint8Array(FILE_KEY_BYTES);
	globalThis.crypto.getRandomValues(fileKey);
	return fileKey;
}

async function readExact(
	source: AttachmentSource,
	offset: number,
	length: number,
): Promise<Uint8Array> {
	const bytes = await source.read(offset, length);
	if (bytes.length !== length) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment source returned the wrong number of bytes',
		);
	}
	return bytes;
}

/**
 * Authenticate and decrypt `age` ciphertext into an injected sink.
 *
 * Ciphertext length and the descriptor's plaintext size and BLAKE3 revision are
 * checked before this resolves. Callers must discard sink contents if this
 * throws; a partial write is not a complete attachment.
 */
export async function decryptAttachmentToSink(
	objectKey: Uint8Array,
	blob: FileChangeBlob,
	source: AttachmentSource,
	sink: AttachmentSink,
): Promise<void> {
	validateAttachmentBlob(blob);
	if (source.size !== blob.size) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobLengthMismatch,
			'attachment ciphertext length did not match its descriptor',
		);
	}
	const { seed, publicKey } = await deriveBlobRecipient(objectKey);
	const prefix = await readExact(
		source,
		0,
		Math.min(blob.size, BLOB_HEADER_BUDGET_BYTES),
	);
	const parsed = await parseAgeHeaderDetailed(prefix, seed, publicKey);
	if (parsed.headerEnd + 16 > blob.size) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobLengthMismatch,
			'attachment ciphertext was truncated',
		);
	}
	const nonce = await readExact(source, parsed.headerEnd, 16);
	const payloadKey = hkdf(sha256, parsed.fileKey, nonce, AGE_PAYLOAD_LABEL, 32);

	const plaintextHash = blake3.create();
	let plaintextSize = 0;
	let offset = parsed.headerEnd + 16;
	let remaining = blob.size - offset;
	if (remaining < BLOB_TAG_BYTES) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobLengthMismatch,
			'attachment ciphertext was truncated',
		);
	}
	let counter = 0;
	let plaintextOffset = 0;
	while (remaining > 0) {
		const length = Math.min(BLOB_ENCRYPTED_CHUNK_BYTES, remaining);
		const chunk = await readExact(source, offset, length);
		const guessedLast = length < BLOB_ENCRYPTED_CHUNK_BYTES;
		let plaintext: Uint8Array;
		try {
			plaintext = chacha20poly1305(
				payloadKey,
				streamNonce(counter, guessedLast),
			).decrypt(chunk);
		} catch (cause) {
			if (guessedLast) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.BlobDecryptFailed,
					'attachment ciphertext failed authentication',
					{ cause },
				);
			}
			// An exact multiple of the STREAM chunk size ends in a full chunk
			// whose last flag the native writer set; retry as the final chunk.
			try {
				plaintext = chacha20poly1305(
					payloadKey,
					streamNonce(counter, true),
				).decrypt(chunk);
			} catch (finalCause) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.BlobDecryptFailed,
					'attachment ciphertext failed authentication',
					{ cause: finalCause },
				);
			}
		}
		await sink.write(plaintextOffset, plaintext);
		plaintextHash.update(plaintext);
		plaintextSize += plaintext.length;
		plaintextOffset += plaintext.length;
		offset += length;
		remaining -= length;
		counter += 1;
	}
	if (
		plaintextSize !== blob.plaintextSize ||
		hex(plaintextHash.digest()) !== blob.revision
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDigestMismatch,
			'attachment plaintext did not match its signed revision',
		);
	}
}

interface ParsedAgeHeader {
	fileKey: Bytes;
	headerEnd: number;
}

async function parseAgeHeaderDetailed(
	ciphertext: Uint8Array,
	seed: Uint8Array,
	publicKey: Uint8Array,
): Promise<ParsedAgeHeader> {
	const budget = Math.min(ciphertext.length, BLOB_HEADER_BUDGET_BYTES);
	const text = latin1(ciphertext.subarray(0, budget));
	if (!text.startsWith(AGE_MAGIC)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment ciphertext did not start with an age header',
		);
	}
	let position = AGE_MAGIC.length;
	const stanzas: AgeStanza[] = [];
	while (true) {
		if (text.startsWith('--- ', position)) break;
		if (!text.startsWith('-> ', position)) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidBlob,
				'attachment header was malformed',
			);
		}
		const lineEnd = text.indexOf('\n', position);
		if (lineEnd < 0) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidBlob,
				'attachment header was truncated',
			);
		}
		const line = text.slice(position + 3, lineEnd);
		const parts = line.split(' ');
		const tag = parts[0] ?? '';
		const args = parts.slice(1);
		position = lineEnd + 1;
		let bodyBase64 = '';
		while (true) {
			const bodyEnd = text.indexOf('\n', position);
			if (bodyEnd < 0) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidBlob,
					'attachment header was truncated',
				);
			}
			const bodyLine = text.slice(position, bodyEnd);
			position = bodyEnd + 1;
			if (bodyLine.length > 64) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidBlob,
					'attachment header was malformed',
				);
			}
			bodyBase64 += bodyLine;
			if (bodyLine.length < 64) break;
		}
		stanzas.push({
			tag,
			args,
			body: decodeBase64NoPad(bodyBase64),
		});
	}
	if (!text.startsWith('--- ', position)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header was malformed',
		);
	}
	const macEnd = text.indexOf('\n', position);
	if (macEnd < 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment header was truncated',
		);
	}
	const mac = decodeBase64NoPad(text.slice(position + 4, macEnd), 32);
	const headerPrefix = ciphertext.subarray(0, position + 3);

	let fileKey: Bytes | null = null;
	for (const stanza of stanzas) {
		if (
			stanza.tag !== AGE_X25519_TAG ||
			stanza.args.length !== 1 ||
			stanza.body.length !== WRAPPED_FILE_KEY_BYTES
		) {
			continue;
		}
		const ephemeralPublic = decodeBase64NoPad(
			stanza.args[0],
			EPHEMERAL_PUBLIC_BYTES,
		);
		const shared = await x25519(seed, ephemeralPublic);
		const salt = new Uint8Array(64);
		salt.set(ephemeralPublic, 0);
		salt.set(publicKey, 32);
		const wrappingKey = hkdf(sha256, shared, salt, AGE_X25519_LABEL, 32);
		try {
			const candidate = chacha20poly1305(
				wrappingKey,
				new Uint8Array(12),
			).decrypt(stanza.body);
			if (candidate.length !== FILE_KEY_BYTES) {
				throw new Error('wrong file key length');
			}
			fileKey = candidate;
			break;
		} catch {
			// This stanza was addressed to a different recipient; try the next.
		}
	}
	if (fileKey === null) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDecryptFailed,
			'attachment ciphertext was not addressed to this object key',
		);
	}
	const expectedMac = hmac(
		sha256,
		hkdf(sha256, fileKey, undefined, AGE_HEADER_LABEL, 32),
		headerPrefix,
	);
	if (!bytesEqual(expectedMac, mac)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDecryptFailed,
			'attachment header authentication failed',
		);
	}
	return { fileKey, headerEnd: macEnd + 1 };
}

/** Verify downloaded ciphertext against its signed SHA-256 digest and size. */
export async function verifyAttachment(
	blob: FileChangeBlob,
	source: AttachmentSource,
): Promise<void> {
	validateAttachmentBlob(blob);
	if (source.size !== blob.size) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDigestMismatch,
			'attachment ciphertext size did not match its digest',
		);
	}
	const hash = sha256.create();
	let offset = 0;
	while (offset < source.size) {
		const length = Math.min(BLOB_UPLOAD_CHUNK_BYTES, source.size - offset);
		hash.update(await readExact(source, offset, length));
		offset += length;
	}
	if (hex(hash.digest()) !== blob.id) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDigestMismatch,
			'attachment ciphertext did not match its signed digest',
		);
	}
}

/** Encrypt in-memory attachment bytes; the caller bounds `plaintext`. */
export async function encryptAttachment(
	objectKey: Uint8Array,
	plaintext: Uint8Array,
): Promise<EncryptedAttachment> {
	const sink = new MemoryAttachmentSink();
	const blob = await encryptAttachmentToSink(
		objectKey,
		bytesAttachmentSource(plaintext),
		sink,
	);
	return { ciphertext: sink.toBytes(), blob };
}

/**
 * Verify and decrypt an in-memory attachment. The signed digest is checked
 * before any authenticated decryption.
 */
export async function decryptAttachment(
	objectKey: Uint8Array,
	blob: FileChangeBlob,
	ciphertext: Uint8Array,
): Promise<Bytes> {
	const source = bytesAttachmentSource(ciphertext);
	await verifyAttachment(blob, source);
	const sink = new MemoryAttachmentSink();
	await decryptAttachmentToSink(objectKey, blob, source, sink);
	return sink.toBytes();
}

/**
 * Build a version-3 file change carrying an encrypted attachment descriptor.
 * The path, move, and base-revision rules are validated when the payload is
 * encoded by `encodeFileChange`.
 */
export function buildAttachmentFileChange(
	input: AttachmentFileChangeInput,
): FileChangeV3 {
	validateAttachmentBlob(input.blob);
	return {
		version: 3,
		path: input.path,
		previousPath: input.previousPath ?? null,
		baseRevision: input.baseRevision ?? null,
		content: null,
		blob: input.blob,
	};
}

function blobRequestError(response: Response): BrowserSyncError {
	if (response.status === 401 || response.status === 403) {
		return new BrowserSyncError(
			BrowserSyncErrorCode.Unauthorized,
			'attachment request was not authorized',
			{ status: response.status },
		);
	}
	if (response.status === 404) {
		return new BrowserSyncError(
			BrowserSyncErrorCode.BlobUnavailable,
			'attachment ciphertext was not found',
			{ status: response.status },
		);
	}
	if (response.status === 413) {
		return new BrowserSyncError(
			BrowserSyncErrorCode.BlobTooLarge,
			'attachment exceeded the server size limit or workspace quota',
			{ status: response.status },
		);
	}
	if (response.status === 416) {
		return new BrowserSyncError(
			BrowserSyncErrorCode.BlobRangeInvalid,
			'attachment byte range was rejected',
			{ status: response.status },
		);
	}
	if (response.status === 409) {
		return new BrowserSyncError(
			BrowserSyncErrorCode.RequestFailed,
			'attachment upload conflicted with the server state',
			{ status: response.status },
		);
	}
	return new BrowserSyncError(
		BrowserSyncErrorCode.RequestFailed,
		`attachment request failed with status ${response.status}`,
		{ status: response.status },
	);
}

function blobUrl(
	origin: string,
	workspaceId: string,
	objectId: string,
	suffix: string,
): string {
	return new URL(
		`/v1/workspaces/${encodeURIComponent(workspaceId)}/objects/${encodeURIComponent(objectId)}/blobs/${suffix}`,
		origin,
	).toString();
}

function blobHeaders(token: string, extra: Record<string, string> = {}) {
	return {
		authorization: `Bearer ${token}`,
		accept: 'application/json',
		...extra,
	};
}

function requireOffset(value: string | null, label: string): number {
	if (value === null || !/^(0|[1-9][0-9]*)$/.test(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`attachment ${label} was malformed`,
		);
	}
	const number = Number(value);
	if (!Number.isSafeInteger(number) || number < 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`attachment ${label} was malformed`,
		);
	}
	return number;
}

/** Inputs for {@link uploadBlob}. */
export interface UploadBlobInput extends AttachmentLocation {
	/** Account origin. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Injected transport. */
	fetch: FetchLike;
	/** Descriptor of the ciphertext being uploaded. */
	blob: FileChangeBlob;
	/** Ciphertext source; read in bounded 1 MiB ranges. */
	source: AttachmentSource;
	/** Optional staged access-transition id the blob belongs to. */
	transitionId?: string;
	/** Optional staged object-activation id the blob belongs to. */
	activationId?: string;
}

/**
 * Upload ciphertext with the server's resumable `tus` protocol.
 *
 * The reservation is created first, the server's current offset is read with a
 * `HEAD`, then at most {@link BLOB_UPLOAD_CHUNK_BYTES} is sent per `PATCH`.
 * Completion is acknowledged only when the server reports
 * `Noura-Blob-Complete: true`; the operation must not be pushed before then.
 */
export async function uploadBlob(input: UploadBlobInput): Promise<void> {
	validateAttachmentBlob(input.blob);
	if (
		!isIdentifier(input.workspaceId) ||
		!isIdentifier(input.objectId) ||
		!Number.isSafeInteger(input.epoch) ||
		input.epoch < 1
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment location was malformed',
		);
	}
	if (input.source.size !== input.blob.size) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBlob,
			'attachment source size did not match its descriptor',
		);
	}
	const createBody: Record<string, unknown> = {
		id: input.blob.id,
		size: input.blob.size,
		epoch: input.epoch,
	};
	if (input.transitionId !== undefined) {
		createBody.transitionId = input.transitionId;
	} else if (input.activationId !== undefined) {
		createBody.activationId = input.activationId;
	}
	const createResponse = await input.fetch(
		blobUrl(input.origin, input.workspaceId, input.objectId, ''),
		{
			method: 'POST',
			credentials: 'include',
			headers: blobHeaders(input.token, { 'content-type': 'application/json' }),
			body: JSON.stringify(createBody),
		},
	);
	if (!createResponse.ok) throw blobRequestError(createResponse);
	const reservation = (await readJson(createResponse)) as {
		id?: unknown;
		offset?: unknown;
		complete?: unknown;
		failed?: unknown;
	};
	if (
		!reservation ||
		typeof reservation !== 'object' ||
		reservation.id !== input.blob.id ||
		typeof reservation.complete !== 'boolean' ||
		typeof reservation.failed !== 'boolean'
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'attachment reservation response was malformed',
		);
	}
	const offset = reservation.offset;
	if (
		typeof offset !== 'number' ||
		!Number.isSafeInteger(offset) ||
		offset < 0 ||
		offset > input.blob.size
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'attachment reservation offset was malformed',
		);
	}
	if (reservation.failed === true) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobUnavailable,
			'attachment reservation previously failed',
		);
	}
	if (reservation.complete === true) {
		if (offset !== input.blob.size) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				'attachment reservation size did not match its descriptor',
			);
		}
		return;
	}

	const path = blobUrl(
		input.origin,
		input.workspaceId,
		input.objectId,
		input.blob.id,
	);
	const head = await input.fetch(path, {
		method: 'HEAD',
		credentials: 'include',
		headers: blobHeaders(input.token, { 'Tus-Resumable': '1.0.0' }),
	});
	if (!head.ok) throw blobRequestError(head);
	let current = requireOffset(head.headers.get('Upload-Offset'), 'offset');
	const length = requireOffset(head.headers.get('Upload-Length'), 'length');
	if (length !== input.blob.size || current > length) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'attachment upload length did not match its descriptor',
		);
	}
	if (current === length) return;

	while (current < length) {
		const size = Math.min(BLOB_UPLOAD_CHUNK_BYTES, length - current);
		const chunk = await readExact(input.source, current, size);
		const response = await input.fetch(path, {
			method: 'PATCH',
			credentials: 'include',
			headers: blobHeaders(input.token, {
				'Tus-Resumable': '1.0.0',
				'Content-Type': 'application/offset+octet-stream',
				'Upload-Offset': String(current),
			}),
			body: chunk as unknown as BodyInit,
		});
		if (!response.ok) throw blobRequestError(response);
		const next = requireOffset(response.headers.get('Upload-Offset'), 'offset');
		if (next !== current + size) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				'attachment upload did not advance to the expected offset',
			);
		}
		current = next;
		if (current === length) {
			if (response.headers.get('Noura-Blob-Complete') !== 'true') {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.BlobUnavailable,
					'attachment upload was not acknowledged as complete',
				);
			}
			return;
		}
	}
}

/** Inputs for {@link downloadBlob}. */
export interface DownloadBlobInput extends AttachmentLocation {
	/** Account origin. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Injected transport. */
	fetch: FetchLike;
	/** Descriptor whose signed digest and size the download is verified against. */
	blob: FileChangeBlob;
	/** Destination; receives bounded 1 MiB ciphertext ranges. */
	sink: AttachmentSink;
}

/**
 * Download ciphertext in bounded ranges and verify the complete ciphertext
 * against its signed SHA-256 digest before this resolves.
 */
export async function downloadBlob(input: DownloadBlobInput): Promise<void> {
	validateAttachmentBlob(input.blob);
	const path = blobUrl(
		input.origin,
		input.workspaceId,
		input.objectId,
		`${input.blob.id}/content`,
	);
	const hash = sha256.create();
	let offset = 0;
	while (offset < input.blob.size) {
		const length = Math.min(BLOB_UPLOAD_CHUNK_BYTES, input.blob.size - offset);
		const end = offset + length - 1;
		const response = await input.fetch(path, {
			method: 'GET',
			credentials: 'include',
			headers: blobHeaders(input.token, {
				Range: `bytes=${offset}-${end}`,
			}),
		});
		if (!response.ok) throw blobRequestError(response);
		if (
			response.status !== 206 ||
			response.headers.get('Content-Range') !==
				`bytes ${offset}-${end}/${input.blob.size}`
		) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				'attachment range response was malformed',
			);
		}
		const bytes = new Uint8Array(await response.arrayBuffer());
		if (bytes.length !== length) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.BlobLengthMismatch,
				'attachment range returned the wrong number of bytes',
			);
		}
		hash.update(bytes);
		await input.sink.write(offset, bytes);
		offset += length;
	}
	if (hex(hash.digest()) !== input.blob.id) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.BlobDigestMismatch,
			'attachment ciphertext did not match its signed digest',
		);
	}
}
