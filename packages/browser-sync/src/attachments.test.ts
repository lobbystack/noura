/**
 * Attachment cryptography, version-3 payload, and transport tests.
 *
 * All tests are in-memory; no network or OPFS is used. The native fixture in
 * `attachments-v1.json` was produced by the Rust `EncryptedBlob::encrypt`
 * (age 0.12.1) and pins byte-compatible decryption.
 */
import { describe, expect, test } from 'bun:test';
import fixture from './attachments-v1.json';
import {
	BLOB_UPLOAD_CHUNK_BYTES,
	MAX_BLOB_BYTES,
	attachmentDigest,
	attachmentRevision,
	buildAttachmentFileChange,
	bytesAttachmentSource,
	decryptAttachment,
	downloadBlob,
	encryptAttachment,
	encryptAttachmentToSink,
	MemoryAttachmentSink,
	uploadBlob,
	validateAttachmentBlob,
	type AttachmentSink,
	type AttachmentSource,
} from './attachments';
import { decodeBase64, encodeBase64, randomBytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { decodeFileChange, encodeFileChange } from './file-change';
import type { FetchLike } from './http';

const objectKey = decodeBase64(fixture.objectKeyBase64);
const nativePlaintext = decodeBase64(fixture.plaintextBase64);
const nativeCiphertext = decodeBase64(fixture.ciphertextBase64);
const nativeBlob = {
	id: fixture.id,
	size: fixture.size,
	plaintextSize: fixture.plaintextSize,
	revision: fixture.revision,
};

function pattern(size: number): Uint8Array {
	const bytes = new Uint8Array(size);
	for (let index = 0; index < size; index += 1) bytes[index] = index % 251;
	return bytes;
}

function expectCode(error: unknown, code: BrowserSyncErrorCode): void {
	expect(error).toBeInstanceOf(BrowserSyncError);
	expect((error as BrowserSyncError).code).toBe(code);
}

describe('attachment cryptography', () => {
	test('decrypts a native age fixture byte-for-byte', async () => {
		expect(nativeBlob.id).toBe(attachmentDigest(nativeCiphertext));
		expect(nativeBlob.revision).toBe(attachmentRevision(nativePlaintext));
		const plaintext = await decryptAttachment(
			objectKey,
			nativeBlob,
			nativeCiphertext,
		);
		expect(Buffer.from(plaintext).equals(Buffer.from(nativePlaintext))).toBe(
			true,
		);
	});

	test('round trips empty, single-byte, boundary, and multi-chunk sizes', async () => {
		for (const size of [0, 1, 65_536, 65_537, 196_608]) {
			const key = randomBytes(32);
			const plaintext = pattern(size);
			const { ciphertext, blob } = await encryptAttachment(key, plaintext);
			expect(blob.size).toBe(ciphertext.length);
			expect(blob.plaintextSize).toBe(size);
			expect(blob.revision).toBe(attachmentRevision(plaintext));
			expect(blob.id).toBe(attachmentDigest(ciphertext));
			expect(blob.size).toBeGreaterThan(blob.plaintextSize);
			validateAttachmentBlob(blob);
			const opened = await decryptAttachment(key, blob, ciphertext);
			expect(Buffer.from(opened).equals(Buffer.from(plaintext))).toBe(true);
		}
	});

	test('rejects tampered ciphertext before decryption', async () => {
		const key = randomBytes(32);
		const { ciphertext, blob } = await encryptAttachment(key, pattern(2048));
		for (const position of [0, ciphertext.length >> 1, ciphertext.length - 1]) {
			const tampered = ciphertext.slice();
			tampered[position] = (tampered[position] ?? 0) ^ 0x01;
			try {
				await decryptAttachment(key, blob, tampered);
				throw new Error('expected rejection');
			} catch (error) {
				expectCode(error, BrowserSyncErrorCode.BlobDigestMismatch);
			}
		}
	});

	test('rejects a wrong object key', async () => {
		const { ciphertext, blob } = await encryptAttachment(
			randomBytes(32),
			pattern(1024),
		);
		try {
			await decryptAttachment(randomBytes(32), blob, ciphertext);
			throw new Error('expected rejection');
		} catch (error) {
			expectCode(error, BrowserSyncErrorCode.BlobDecryptFailed);
		}
	});

	test('rejects a mismatched revision or plaintext size', async () => {
		const key = randomBytes(32);
		const { ciphertext, blob } = await encryptAttachment(key, pattern(100));
		for (const wrong of [
			{ ...blob, revision: '0'.repeat(64) },
			{ ...blob, plaintextSize: blob.plaintextSize + 1 },
		]) {
			try {
				await decryptAttachment(key, wrong, ciphertext);
				throw new Error('expected rejection');
			} catch (error) {
				expectCode(error, BrowserSyncErrorCode.BlobDigestMismatch);
			}
		}
	});

	test('rejects descriptors that violate the size rules', () => {
		for (const blob of [
			{ ...nativeBlob, size: 0 },
			{ ...nativeBlob, size: MAX_BLOB_BYTES + 1 },
			{ ...nativeBlob, plaintextSize: nativeBlob.size },
			{ ...nativeBlob, id: 'not-a-digest' },
			{ ...nativeBlob, revision: 'A'.repeat(64) },
		]) {
			expect(() => validateAttachmentBlob(blob)).toThrow(BrowserSyncError);
		}
	});

	test('bounds plaintext before reading any bytes', async () => {
		let reads = 0;
		const source: AttachmentSource = {
			size: MAX_BLOB_BYTES,
			async read() {
				reads += 1;
				return new Uint8Array();
			},
		};
		const sink = new MemoryAttachmentSink();
		try {
			await encryptAttachmentToSink(randomBytes(32), source, sink);
			throw new Error('expected rejection');
		} catch (error) {
			expectCode(error, BrowserSyncErrorCode.BlobTooLarge);
		}
		expect(reads).toBe(0);
	});

	test('bounds source reads to the requested length', async () => {
		const source = bytesAttachmentSource(pattern(10));
		expect(await source.read(2, 3)).toEqual(new Uint8Array([2, 3, 4]));
		await expect(source.read(8, 3)).rejects.toBeInstanceOf(BrowserSyncError);
		expect(source.size).toBe(10);
	});

	test('builds and validates a version-3 file change', () => {
		const change = buildAttachmentFileChange({
			path: 'assets/picture.png',
			blob: nativeBlob,
		});
		expect(change.version).toBe(3);
		expect(change.content).toBeNull();
		const decoded = decodeFileChange(encodeFileChange(change));
		expect(decoded).toEqual(change);
		const moved = buildAttachmentFileChange({
			path: 'assets/moved.png',
			previousPath: 'assets/picture.png',
			baseRevision: null,
			blob: nativeBlob,
		});
		expect(decodeFileChange(encodeFileChange(moved))).toEqual(moved);
	});
});

/** Minimal tus/range server used by the transport tests. */
class FakeBlobServer {
	readonly origin = 'https://sync.example';
	readonly token = 'token';
	readonly workspaceId = 'workspace';
	readonly objectId = 'object';
	readonly epoch = 1;
	bytes: Uint8Array | null = null;
	storedId: string | null = null;
	readonly patches: number[] = [];
	createBody: Record<string, unknown> | null = null;
	resumeOffset = 0;
	fail: number | null = null;

	readonly fetch: FetchLike = async (input, init) => {
		const url = new URL(
			typeof input === 'string'
				? input
				: input instanceof URL
					? input.toString()
					: input.url,
		);
		const method = init?.method ?? 'GET';
		const match = url.pathname.match(/\/blobs\/([0-9a-f]{64})(\/content)?$/);
		if (method === 'POST') {
			this.createBody = JSON.parse(String(init?.body));
			if (this.fail === 201) return this.errorResponse(500);
			return Response.json(
				{
					id: this.createBody!.id,
					offset: this.resumeOffset,
					complete: false,
					failed: false,
				},
				{ status: 201 },
			);
		}
		if (method === 'HEAD') {
			const size = this.bytes?.length ?? 0;
			return new Response(null, {
				status: 200,
				headers: {
					'Upload-Offset': String(this.resumeOffset),
					'Upload-Length': String(
						(this.createBody?.size as number | undefined) ?? size,
					),
				},
			});
		}
		if (method === 'PATCH') {
			const offset = Number(new Headers(init?.headers).get('Upload-Offset'));
			const body = new Uint8Array(init?.body as ArrayBuffer);
			this.patches.push(body.length);
			if (this.fail === this.patches.length) return this.errorResponse(500);
			const total = this.createBody!.size as number;
			const next = offset + body.length;
			if (this.bytes === null) this.bytes = new Uint8Array(total);
			this.bytes.set(body, offset);
			this.resumeOffset = next;
			return new Response(null, {
				status: 204,
				headers: {
					'Upload-Offset': String(next),
					...(next === total ? { 'Noura-Blob-Complete': 'true' } : {}),
				},
			});
		}
		if (method === 'GET' && match?.[2]) {
			const id = match[1]!;
			const range = new Headers(init?.headers)
				.get('Range')
				?.match(/^bytes=(\d+)-(\d+)$/);
			if (!range || this.bytes === null || id !== this.storedId) {
				return this.errorResponse(404);
			}
			const start = Number(range[1]);
			const end = Number(range[2]);
			const slice = this.bytes.slice(start, end + 1);
			if (this.fail === start) slice[0] = (slice[0] ?? 0) ^ 0x01;
			return new Response(slice, {
				status: 206,
				headers: {
					'Content-Range': `bytes ${start}-${end}/${this.bytes.length}`,
					'Content-Length': String(slice.length),
				},
			});
		}
		return this.errorResponse(404);
	};

	private errorResponse(status: number): Response {
		return Response.json({ error: { code: 'sync.server_error' } }, { status });
	}
}

describe('attachment transport', () => {
	test('uploads ciphertext in bounded patches and verifies completion', async () => {
		const server = new FakeBlobServer();
		const key = randomBytes(32);
		const { ciphertext, blob } = await encryptAttachment(
			key,
			pattern(2 * BLOB_UPLOAD_CHUNK_BYTES + 123),
		);
		await uploadBlob({
			origin: server.origin,
			token: server.token,
			fetch: server.fetch,
			workspaceId: server.workspaceId,
			objectId: server.objectId,
			epoch: server.epoch,
			blob,
			source: bytesAttachmentSource(ciphertext),
		});
		expect(server.createBody).toEqual({
			id: blob.id,
			size: blob.size,
			epoch: server.epoch,
		});
		expect(Math.max(...server.patches)).toBeLessThanOrEqual(
			BLOB_UPLOAD_CHUNK_BYTES,
		);
		expect(server.bytes).not.toBeNull();
		expect(attachmentDigest(server.bytes!)).toBe(blob.id);
	});

	test('resumes an existing upload from the server offset', async () => {
		const server = new FakeBlobServer();
		const { ciphertext, blob } = await encryptAttachment(
			randomBytes(32),
			pattern(4096),
		);
		server.bytes = new Uint8Array(blob.size);
		server.storedId = blob.id;
		server.bytes.set(ciphertext.subarray(0, 1024), 0);
		server.resumeOffset = 1024;
		await uploadBlob({
			origin: server.origin,
			token: server.token,
			fetch: server.fetch,
			workspaceId: server.workspaceId,
			objectId: server.objectId,
			epoch: server.epoch,
			blob,
			source: bytesAttachmentSource(ciphertext),
		});
		expect(server.patches[0]).toBe(blob.size - 1024);
		expect(attachmentDigest(server.bytes)).toBe(blob.id);
	});

	test('surfaces a structured error when an upload patch fails', async () => {
		const server = new FakeBlobServer();
		const { ciphertext, blob } = await encryptAttachment(
			randomBytes(32),
			pattern(4096),
		);
		server.fail = 1;
		try {
			await uploadBlob({
				origin: server.origin,
				token: server.token,
				fetch: server.fetch,
				workspaceId: server.workspaceId,
				objectId: server.objectId,
				epoch: server.epoch,
				blob,
				source: bytesAttachmentSource(ciphertext),
			});
			throw new Error('expected rejection');
		} catch (error) {
			expectCode(error, BrowserSyncErrorCode.RequestFailed);
		}
	});

	test('downloads bounded ranges and verifies the signed digest', async () => {
		const server = new FakeBlobServer();
		const { ciphertext, blob } = await encryptAttachment(
			randomBytes(32),
			pattern(2 * BLOB_UPLOAD_CHUNK_BYTES + 7),
		);
		server.bytes = ciphertext;
		server.storedId = blob.id;
		const sink = new MemoryAttachmentSink();
		await downloadBlob({
			origin: server.origin,
			token: server.token,
			fetch: server.fetch,
			workspaceId: server.workspaceId,
			objectId: server.objectId,
			epoch: server.epoch,
			blob,
			sink,
		});
		expect(Buffer.from(sink.toBytes()).equals(Buffer.from(ciphertext))).toBe(
			true,
		);
	});

	test('rejects a tampered download range', async () => {
		const server = new FakeBlobServer();
		const { ciphertext, blob } = await encryptAttachment(
			randomBytes(32),
			pattern(4096),
		);
		server.bytes = ciphertext;
		server.storedId = blob.id;
		server.fail = 0;
		const sink: AttachmentSink = { async write() {} };
		try {
			await downloadBlob({
				origin: server.origin,
				token: server.token,
				fetch: server.fetch,
				workspaceId: server.workspaceId,
				objectId: server.objectId,
				epoch: server.epoch,
				blob,
				sink,
			});
			throw new Error('expected rejection');
		} catch (error) {
			expectCode(error, BrowserSyncErrorCode.BlobDigestMismatch);
		}
	});

	test('rejects a non-canonical or out-of-range descriptor', async () => {
		const server = new FakeBlobServer();
		try {
			await downloadBlob({
				origin: server.origin,
				token: server.token,
				fetch: server.fetch,
				workspaceId: server.workspaceId,
				objectId: server.objectId,
				epoch: server.epoch,
				blob: { ...nativeBlob, size: 0 },
				sink: { async write() {} },
			});
			throw new Error('expected rejection');
		} catch (error) {
			expectCode(error, BrowserSyncErrorCode.InvalidBlob);
		}
	});
});

test('fixture uses canonical base64 fields', () => {
	expect(encodeBase64(decodeBase64(fixture.ciphertextBase64))).toBe(
		fixture.ciphertextBase64,
	);
});
