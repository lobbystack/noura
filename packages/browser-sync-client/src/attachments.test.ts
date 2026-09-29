import { describe, expect, test } from 'bun:test';
import {
	BrowserSyncErrorCode,
	buildAttachmentFileChange,
	createDeviceIdentity,
	createMemoryKeyStore,
	encodeFileChange,
	encryptAttachment,
	sealOperation,
	unlockDeviceIdentity,
	type DeviceIdentity,
	type FetchLike,
} from '@noura/browser-sync';
import {
	BrowserSyncEngineErrorCode,
	createEmptySyncState,
	createMemorySyncStateStore,
	MemorySyncStorage,
	type BrowserSyncRemote,
} from '@noura/browser-sync-engine';
import type { EncryptedOperation, SequencedOperation } from '@noura/shared';
import {
	createBrowserAttachmentFetcher,
	MAX_BROWSER_ATTACHMENT_BYTES,
} from './attachment-fetcher';
import { runBrowserSyncSendAttachment } from './attachments';
import { createBrowserSyncController } from './controller';
import { runBrowserSyncReconcile } from './reconcile';
import { encoder, ORIGIN, PASSPHRASE, requestUrl } from './test-support';
import type { BrowserSyncWorkspaceBinding } from './types';

describe('browser sync attachments', () => {
	async function attachmentRemote() {
		const workspaceId = 'ws_attachment';
		const objectId = 'obj_attachment';
		const objectKey = crypto.getRandomValues(new Uint8Array(32));
		const remoteIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		// The receiving replica is a different device than the operation author,
		// so it does not skip the operation as its own.
		const localIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const plaintext = encoder.encode('attachment-bytes');
		const { ciphertext, blob } = await encryptAttachment(objectKey, plaintext);
		const payload = encodeFileChange(
			buildAttachmentFileChange({ path: 'assets/report.bin', blob }),
		);
		const operation = await sealOperation({
			objectKey,
			workspaceId,
			objectId,
			deviceId: remoteIdentity.deviceId,
			epoch: 1,
			policyRevision: '1',
			plaintext: payload,
			identity: remoteIdentity,
		});
		const remote: BrowserSyncRemote = {
			async push(operations) {
				return { sequences: operations.map((_, index) => String(index + 1)) };
			},
			async pull(cursor) {
				if (cursor === '0') {
					return {
						accessRevision: '1',
						operations: [{ ...operation, sequence: '1' }],
						cursor: '1',
						hasMore: false,
					};
				}
				return {
					accessRevision: '1',
					operations: [],
					cursor,
					hasMore: false,
				};
			},
		};
		const fetch: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			const match = url.match(/\/blobs\/([0-9a-f]{64})\/content$/);
			const range = new Headers(init?.headers)
				.get('Range')
				?.match(/^bytes=(\d+)-(\d+)$/);
			if (init?.method === 'GET' && match && range) {
				const start = Number(range[1]);
				const end = Number(range[2]);
				return new Response(ciphertext.slice(start, end + 1), {
					status: 206,
					headers: {
						'Content-Range': `bytes ${start}-${end}/${ciphertext.length}`,
					},
				});
			}
			throw new Error(`unexpected attachment request in test: ${url}`);
		};
		return {
			workspaceId,
			objectId,
			objectKey,
			remoteIdentity,
			localIdentity,
			plaintext,
			blob,
			ciphertext,
			remote,
			fetch,
		};
	}

	function reconcileInput(
		setup: Awaited<ReturnType<typeof attachmentRemote>>,
		storage: MemorySyncStorage,
		withAttachments: boolean,
	) {
		return {
			identity: setup.localIdentity,
			workspaceId: setup.workspaceId,
			objectId: setup.objectId,
			epoch: 1,
			policyRevision: '1',
			objectKeys: new Map([[setup.objectId, setup.objectKey]]),
			pinnedSigners: new Map([
				[setup.remoteIdentity.deviceId, setup.remoteIdentity.signingPublic],
			]),
			storage,
			state: createMemorySyncStateStore(),
			remote: setup.remote,
			...(withAttachments
				? {
						attachments: {
							origin: ORIGIN,
							token: 'token-attachment',
							fetch: setup.fetch,
						},
					}
				: {}),
		};
	}

	test('applies a version-3 change by downloading and decrypting the blob', async () => {
		const setup = await attachmentRemote();
		const storage = new MemorySyncStorage();

		const result = await runBrowserSyncReconcile(
			reconcileInput(setup, storage, true),
		);

		expect(result.applied).toBe(1);
		const stored = await storage.read('assets/report.bin');
		expect(stored).not.toBeNull();
		expect(
			Buffer.from(stored!.bytes).equals(Buffer.from(setup.plaintext)),
		).toBe(true);
	});

	test('without transport metadata, a version-3 change is not written', async () => {
		const setup = await attachmentRemote();
		const storage = new MemorySyncStorage();

		await expect(
			runBrowserSyncReconcile(reconcileInput(setup, storage, false)),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.AttachmentUnavailable,
		});
		expect(await storage.read('assets/report.bin')).toBeNull();
	});

	test('refuses an attachment larger than the browser memory bound', async () => {
		const setup = await attachmentRemote();
		const fetcher = createBrowserAttachmentFetcher({
			origin: ORIGIN,
			token: 'token-attachment',
			fetch: setup.fetch,
			workspaceId: setup.workspaceId,
			objectKeys: new Map([[setup.objectId, setup.objectKey]]),
			maxBytes: 8,
		});
		await expect(
			fetcher.fetch({
				workspaceId: setup.workspaceId,
				objectId: setup.objectId,
				epoch: 1,
				path: 'assets/report.bin',
				previousPath: null,
				baseRevision: null,
				content: null,
				blob: setup.blob,
			}),
		).rejects.toMatchObject({ code: 'browser_sync_blob_too_large' });
		expect(MAX_BROWSER_ATTACHMENT_BYTES).toBeGreaterThan(8);
	});
});

describe('browser attachment send', () => {
	const sendObjectKey = new Uint8Array(32).fill(9);
	const sendOtherKey = new Uint8Array(32).fill(11);
	const sendWorkspaceId = 'workspace';
	const sendObjectId = 'obj_note';
	const sendPath = 'attachments/obj_note/photo.png';

	function sendPattern(size: number): Uint8Array {
		const bytes = new Uint8Array(size);
		for (let index = 0; index < size; index += 1) bytes[index] = index % 251;
		return bytes;
	}

	class FakeSendRemote implements BrowserSyncRemote {
		readonly pushed: EncryptedOperation[] = [];

		async push(
			operations: EncryptedOperation[],
		): Promise<{ sequences: string[] }> {
			this.pushed.push(...operations);
			return {
				sequences: operations.map((_, index) =>
					String(this.pushed.length * 1000 + index),
				),
			};
		}

		async pull(cursor: string) {
			return {
				accessRevision: cursor,
				cursor,
				hasMore: false,
				operations: this.pushed.map((operation, index) => ({
					...operation,
					sequence: String(index),
				})) as SequencedOperation[],
			};
		}
	}

	/** Minimal tus/range blob server; no network is used. */
	class FakeBlobServer {
		readonly origin = ORIGIN;
		readonly token = 'token-attachment';
		bytes: Uint8Array | null = null;
		storedId: string | null = null;
		createBody: Record<string, unknown> | null = null;
		resumeOffset = 0;
		fail: number | null = null;
		readonly patches: number[] = [];

		readonly fetch: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			const method = init?.method ?? 'GET';
			const match = url.match(/\/blobs\/([0-9a-f]{64})(\/content)?$/);
			if (method === 'POST') {
				this.createBody = JSON.parse(String(init?.body)) as Record<
					string,
					unknown
				>;
				return Response.json(
					{
						id: this.createBody.id,
						offset: this.resumeOffset,
						complete: false,
						failed: false,
					},
					{ status: 201 },
				);
			}
			if (method === 'HEAD') {
				return new Response(null, {
					status: 200,
					headers: {
						'Upload-Offset': String(this.resumeOffset),
						'Upload-Length': String(
							(this.createBody?.size as number | undefined) ??
								this.bytes?.length ??
								0,
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
				if (!range || this.bytes === null || id !== this.storedId)
					return this.errorResponse(404);
				const start = Number(range[1]);
				const end = Number(range[2]);
				const slice = this.bytes.slice(start, end + 1);
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
			return Response.json(
				{ error: { code: 'sync.server_error' } },
				{ status },
			);
		}
	}

	function sendBinding(
		identity: DeviceIdentity,
		options: {
			storage?: MemorySyncStorage;
			remote?: BrowserSyncRemote;
			server?: FakeBlobServer;
			key?: Uint8Array;
		} = {},
	): BrowserSyncWorkspaceBinding {
		const server = options.server ?? new FakeBlobServer();
		return {
			workspaceId: sendWorkspaceId,
			objectId: sendObjectId,
			epoch: 1,
			policyRevision: '1',
			objectKeys: new Map([[sendObjectId, options.key ?? sendObjectKey]]),
			objects: new Map([
				[
					sendObjectId,
					{
						objectId: sendObjectId,
						path: 'Notes/a.md',
						localObjectId: 'note_a',
						epoch: 1,
						policyRevision: '1',
					},
				],
			]),
			pinnedSigners: new Map([[identity.deviceId, identity.signingPublic]]),
			storage: options.storage ?? new MemorySyncStorage(),
			state: createMemorySyncStateStore(),
			remote: options.remote ?? new FakeSendRemote(),
			attachments: {
				origin: server.origin,
				token: server.token,
				fetch: server.fetch,
			},
		};
	}

	async function sendIdentity(): Promise<DeviceIdentity> {
		return unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
	}

	test('encrypts, uploads, seals v3, and a fresh replica applies it', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(2 * 1024 * 1024 + 123);

		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: plaintext,
		});
		expect(result.path).toBe(sendPath);
		expect(server.bytes).not.toBeNull();
		expect(server.patches.every((size) => size <= 1024 * 1024)).toBe(true);

		const local = await sender.storage.read(result.path);
		expect(local).not.toBeNull();
		expect(Buffer.from(local!.bytes).equals(Buffer.from(plaintext))).toBe(true);

		const queued = await sender.state.read();
		expect(queued.outbox).toHaveLength(1);
		expect(queued.knownPaths).toContain(result.path);

		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });
		expect(remote.pushed).toHaveLength(1);
		expect(remote.pushed[0]!.objectId).toBe(sendObjectId);

		// A fresh replica with empty storage and durable state applies the queued
		// version-3 operation, fetching and decrypting the attachment.
		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
		});
		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(1);
		const stored = await receiverStorage.read(result.path);
		expect(stored).not.toBeNull();
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(plaintext))).toBe(
			true,
		);
	});

	test('a reloaded replica applies the persisted version-3 operation', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(4096);
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'notes.bin',
			bytes: plaintext,
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		// A new replica instance with empty storage reopens the encrypted
		// operation from the remote and materializes the attachment.
		const storage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, { storage, remote, server });
		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(1);
		const stored = await storage.read(result.path);
		expect(stored).not.toBeNull();
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(plaintext))).toBe(
			true,
		);
	});

	test('refuses a plaintext above the browser bound without uploading', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const sender = sendBinding(identity, { server });

		await expect(
			runBrowserSyncSendAttachment({
				...sender,
				identity,
				objectId: sendObjectId,
				name: 'big.bin',
				bytes: sendPattern(9),
				maxBytes: 8,
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.BlobTooLarge });

		expect(server.createBody).toBeNull();
		const state = await sender.state.read();
		expect(state.outbox).toHaveLength(0);
		expect(
			await sender.storage.read('attachments/obj_note/big.bin'),
		).toBeNull();
	});

	test('an upload failure leaves no operation enqueued', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		server.fail = 1;
		const sender = sendBinding(identity, { server });

		await expect(
			runBrowserSyncSendAttachment({
				...sender,
				identity,
				objectId: sendObjectId,
				name: 'photo.png',
				bytes: sendPattern(4096),
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.RequestFailed });

		const state = await sender.state.read();
		expect(state.outbox).toHaveLength(0);
		expect(await sender.storage.read(sendPath)).toBeNull();
	});

	test('a tampered uploaded blob is rejected and nothing is written', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: sendPattern(2048),
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });
		expect(server.bytes).not.toBeNull();
		server.bytes![0] = (server.bytes![0] ?? 0) ^ 0x01;

		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
		});
		await expect(
			runBrowserSyncReconcile({ ...receiver, identity: receiverIdentity }),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.AttachmentUnavailable,
		});
		expect(await receiverStorage.read(result.path)).toBeNull();
	});

	test('a wrong object key cannot open the queued operation', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: sendPattern(1024),
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
			key: sendOtherKey,
		});
		await expect(
			runBrowserSyncReconcile({ ...receiver, identity: receiverIdentity }),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidOperation,
		});
		expect(await receiverStorage.read(result.path)).toBeNull();
	});

	test('keeps version-1 conflict rules for a version-3 change', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(512);
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: plaintext,
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		const storage = new MemorySyncStorage();
		const local = new Uint8Array([1, 2, 3, 4]);
		const written = await storage.write({
			path: result.path,
			bytes: local,
			expectedRevision: null,
		});
		// Seed the baseline so the local attachment is not re-sealed; the queued
		// remote version-3 change then hits the ordinary revision rules.
		const state = createMemorySyncStateStore({
			...createEmptySyncState(),
			pushedRevisions: { [result.path]: written.revision },
			knownPaths: [result.path],
		});
		const receiver = {
			...sendBinding(identity, { storage, remote, server }),
			state,
		};
		const receiverIdentity = await sendIdentity();

		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(0);
		expect(outcome.conflicts).toHaveLength(1);
		expect(outcome.conflicts[0]!.reason).toBe('unexpected_file');
		const stored = await storage.read(result.path);
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(local))).toBe(true);
	});
});

describe('controller attach and sync', () => {
	test('refuses to attach without custody and never reports an upload', async () => {
		const unavailable = await createBrowserSyncController({ keyStore: null });
		const refused = await unavailable.attachAndSync({
			noteId: 'note_one',
			name: 'photo.png',
			bytes: new Uint8Array([1, 2, 3]),
		});
		expect(refused.ok).toBe(false);
		if (!refused.ok) expect(refused.code).toBe('unavailable');

		let uploaded = false;
		const locked = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
		});
		const lockedResult = await locked.attachAndSync({
			noteId: 'note_one',
			name: 'photo.png',
			bytes: new Uint8Array([1, 2, 3]),
			onUploaded: () => {
				uploaded = true;
			},
		});
		expect(lockedResult.ok).toBe(false);
		if (!lockedResult.ok) expect(lockedResult.code).toBe('locked');
		expect(uploaded).toBe(false);
	});
});
