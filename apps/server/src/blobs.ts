import { Server, Upload } from '@tus/server';
import { FileStore } from '@tus/file-store';
import { createHash, randomBytes } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { access, lstat, mkdir, open, realpath, unlink } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import type postgres from 'postgres';
import { identifier, record, SyncError } from './protocol';
import type { Actor, SyncStore } from './store';

const MAX_BLOB = 1024 * 1024 * 1024;
const MAX_PATCH = 1024 * 1024;
type Tx = postgres.TransactionSql;

export class BlobService {
	private constructor(
		readonly store: SyncStore,
		readonly directory: string,
		readonly s3?: Bun.S3Client,
	) {}
	static async open(store: SyncStore, directory: string, s3?: Bun.S3Client) {
		await mkdir(resolve(directory), { recursive: true, mode: 0o700 });
		return new BlobService(store, await realpath(directory), s3);
	}
	async ready() {
		await access(this.directory, 6);
	}

	private path(upload: string) {
		if (!/^[0-9a-f]{32}$/.test(upload))
			throw new SyncError('sync.invalid_upload');
		return join(this.directory, upload);
	}
	private async durable(upload: string) {
		const path = this.path(upload);
		const info = await lstat(path);
		if (!info.isFile() || info.isSymbolicLink())
			throw new SyncError('sync.invalid_upload');
		const file = await open(path, 'r+');
		try {
			await file.sync();
		} finally {
			await file.close();
		}
		const parent = await open(this.directory, 'r');
		try {
			await parent.sync();
		} finally {
			await parent.close();
		}
		return info.size;
	}
	private objectKey(workspace: string, object: string, id: string) {
		return `blobs/${workspace}/${object}/${id}`;
	}
	private async authorize(
		tx: Tx,
		actor: Actor,
		workspace: string,
		object: string,
		role: string | undefined,
		write: boolean,
	) {
		const [grant] =
			await tx`SELECT role FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${object} AND account_id=${actor.accountId}`;
		if (
			write
				? (!role || role === 'viewer') && grant?.role !== 'editor'
				: !role && !grant
		)
			throw new SyncError('sync.forbidden', 403);
		const [entry] =
			await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${workspace} AND id=${object}`;
		if (!entry) throw new SyncError('sync.not_found', 404);
		return Number(entry.epoch);
	}
	private tus(tx: Tx, row: postgres.Row) {
		const upload = row.upload_id as string;
		const configstore = {
			get: async (id: string) =>
				id === upload && row.tus_info ? new Upload(row.tus_info) : undefined,
			set: async (id: string, value: Upload) => {
				if (id !== upload) throw new SyncError('sync.invalid_upload');
				row.tus_info = JSON.parse(JSON.stringify(value));
				await tx`UPDATE noura_blobs SET tus_info=${tx.json(row.tus_info)} WHERE upload_id=${upload}`;
			},
			delete: async () => {
				throw new SyncError('sync.forbidden', 403);
			},
		};
		return new Server({
			path: '/uploads',
			datastore: new FileStore({ directory: this.directory, configstore }),
			maxSize: MAX_BLOB,
			relativeLocation: true,
			allowedOrigins: [],
			respectForwardedHeaders: false,
			disableTerminationForFinishedUploads: true,
			namingFunction: () => upload,
			onResponseError: async (_request, error) => ({
				status_code: 'status_code' in error ? error.status_code : 500,
				body: 'Encrypted upload could not complete',
			}),
		});
	}

	/** SQL serializes each workspace while tus owns offset validation and file transfer. */
	async create(
		actor: Actor,
		workspace: string,
		object: string,
		input: unknown,
	) {
		identifier(workspace);
		identifier(object);
		const body = record(input);
		if (
			Object.keys(body).sort().join(',') !== 'epoch,id,size' ||
			typeof body.id !== 'string' ||
			!/^[0-9a-f]{64}$/.test(body.id) ||
			!Number.isSafeInteger(body.epoch) ||
			(body.epoch as number) < 1 ||
			!Number.isSafeInteger(body.size) ||
			(body.size as number) < 1 ||
			(body.size as number) > MAX_BLOB
		)
			throw new SyncError('sync.invalid_blob');
		const id = body.id;
		const size = body.size as number;
		const epoch = body.epoch as number;
		return this.store.withWorkspace(
			actor,
			workspace,
			async (tx, state, role) => {
				if (
					(await this.authorize(tx, actor, workspace, object, role, true)) !==
					epoch
				)
					throw new SyncError('sync.stale_epoch', 409);
				const [existing] =
					await tx`SELECT * FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${object} AND id=${id}`;
				if (existing) {
					if (
						Number(existing.size) !== size ||
						Number(existing.epoch) !== epoch ||
						existing.device_id !== actor.deviceId
					)
						throw new SyncError('sync.blob_changed', 409);
					return {
						id,
						offset: existing.complete
							? size
							: await this.durable(existing.upload_id),
						complete: existing.complete,
						failed: existing.failed,
					};
				}
				if (BigInt(state.used_bytes) + BigInt(size) > BigInt(state.quota_bytes))
					throw new SyncError('sync.quota_exceeded', 413);
				const upload = randomBytes(16).toString('hex');
				const [row] =
					await tx`INSERT INTO noura_blobs(workspace_id,object_id,id,epoch,size,upload_id,device_id) VALUES(${workspace},${object},${id},${epoch},${size},${upload},${actor.deviceId}) RETURNING *`;
				await tx`UPDATE noura_workspaces SET used_bytes=used_bytes+${size} WHERE id=${workspace}`;
				const response = await this.tus(tx, row!).handleWeb(
					new Request('http://localhost/uploads', {
						method: 'POST',
						headers: {
							'Tus-Resumable': '1.0.0',
							'Upload-Length': String(size),
						},
					}),
				);
				if (response.status !== 201)
					throw new SyncError('sync.upload_unavailable', 503);
				await this.durable(upload);
				return { id, offset: 0, complete: false, failed: false };
			},
		);
	}

	async transfer(
		actor: Actor,
		workspace: string,
		object: string,
		id: string,
		request: Request,
	): Promise<Response> {
		identifier(workspace);
		identifier(object);
		if (!/^[0-9a-f]{64}$/.test(id)) throw new SyncError('sync.invalid_blob');
		if (!['HEAD', 'PATCH', 'DELETE'].includes(request.method))
			throw new SyncError('sync.forbidden', 403);
		// Buffer one bounded encrypted patch before taking the workspace lock.
		const body =
			request.method === 'PATCH'
				? new Uint8Array(await request.arrayBuffer())
				: undefined;
		if (body && body.length > MAX_PATCH)
			throw new SyncError('sync.request_too_large', 413);
		let finish: { upload: string; size: number; epoch: number } | undefined;
		const response = await this.store.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				const epoch = await this.authorize(
					tx,
					actor,
					workspace,
					object,
					role,
					true,
				);
				const [row] =
					await tx`SELECT * FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${object} AND id=${id}`;
				if (!row) throw new SyncError('sync.not_found', 404);
				if (row.device_id !== actor.deviceId)
					throw new SyncError('sync.forbidden', 403);
				if (row.complete) {
					if (request.method !== 'HEAD')
						throw new SyncError('sync.blob_immutable', 409);
					return new Response(null, {
						headers: {
							'Tus-Resumable': '1.0.0',
							'Upload-Offset': String(row.size),
							'Upload-Length': String(row.size),
							'Noura-Blob-Complete': 'true',
						},
					});
				}
				if (request.method === 'DELETE') {
					await unlink(this.path(row.upload_id)).catch(
						(error: NodeJS.ErrnoException) => {
							if (error.code !== 'ENOENT') throw error;
						},
					);
					const directory = await open(this.directory, 'r');
					try {
						await directory.sync();
					} finally {
						await directory.close();
					}
					await tx`DELETE FROM noura_blobs WHERE upload_id=${row.upload_id}`;
					await tx`UPDATE noura_workspaces SET used_bytes=used_bytes-${row.size} WHERE id=${workspace}`;
					return new Response(null, { status: 204 });
				}
				if (Number(row.epoch) !== epoch)
					throw new SyncError('sync.stale_epoch', 409);
				if (row.failed) throw new SyncError('sync.blob_digest_mismatch', 409);
				const headers = new Headers(request.headers);
				const internal = new Request(
					`http://localhost/uploads/${row.upload_id}`,
					{ method: request.method, headers, ...(body ? { body } : {}) },
				);
				const response = await this.tus(tx, row).handleWeb(internal);
				if (!response.ok) return response;
				const offset = await this.durable(row.upload_id);
				if (offset === Number(row.size)) {
					finish = { upload: row.upload_id, size: Number(row.size), epoch };
				}
				return response;
			},
		);
		if (finish) {
			await this.finalize(actor, workspace, object, id, finish);
			response.headers.set('Noura-Blob-Complete', 'true');
		}
		return response;
	}

	/** Complete immutable ciphertext outside the workspace lock so file operations remain responsive. */
	private async finalize(
		actor: Actor,
		workspace: string,
		object: string,
		id: string,
		finish: { upload: string; size: number; epoch: number },
	) {
		const hash = createHash('sha256');
		for await (const chunk of createReadStream(this.path(finish.upload)))
			hash.update(chunk);
		const valid = hash.digest('hex') === id;
		if (valid && this.s3) {
			await this.s3.write(
				this.objectKey(workspace, object, id),
				Bun.file(this.path(finish.upload)),
			);
		}
		// Recheck authorization and upload identity after hashing/storage. A concurrent delete,
		// recreate, access change or revocation cannot turn an old upload into a published blob.
		await this.store.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				if (
					(await this.authorize(tx, actor, workspace, object, role, true)) !==
					finish.epoch
				)
					throw new SyncError('sync.stale_epoch', 409);
				const [row] =
					await tx`SELECT * FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${object} AND id=${id}`;
				if (
					!row ||
					row.upload_id !== finish.upload ||
					row.device_id !== actor.deviceId ||
					Number(row.size) !== finish.size
				)
					throw new SyncError('sync.blob_changed', 409);
				if (valid) {
					await tx`UPDATE noura_blobs SET complete=true,storage=${this.s3 ? 's3' : 'local'} WHERE upload_id=${finish.upload}`;
				} else {
					await tx`UPDATE noura_blobs SET failed=true WHERE upload_id=${finish.upload}`;
				}
			},
		);
		if (!valid) throw new SyncError('sync.blob_digest_mismatch', 409);
	}

	async read(
		actor: Actor,
		workspace: string,
		object: string,
		id: string,
		range: string | undefined,
	) {
		identifier(workspace);
		identifier(object);
		if (!/^[0-9a-f]{64}$/.test(id)) throw new SyncError('sync.invalid_blob');
		const selected = await this.store.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				await this.authorize(tx, actor, workspace, object, role, false);
				const [row] =
					await tx`SELECT * FROM noura_blobs WHERE workspace_id=${workspace} AND object_id=${object} AND id=${id} AND complete`;
				if (!row) throw new SyncError('sync.not_found', 404);
				const match = range?.match(/^bytes=(0|[1-9][0-9]*)-(0|[1-9][0-9]*)$/);
				if (!match) throw new SyncError('sync.invalid_blob_range', 416);
				const start = Number(match[1]);
				const end = Number(match[2]);
				const size = Number(row.size);
				if (
					!Number.isSafeInteger(start) ||
					!Number.isSafeInteger(end) ||
					start > end ||
					end >= size ||
					end - start + 1 > MAX_PATCH
				)
					throw new SyncError('sync.invalid_blob_range', 416);
				return {
					start,
					end,
					size,
					storage: row.storage,
					upload: row.upload_id as string,
				};
			},
		);
		const { start, end, size } = selected;
		let bytes: Uint8Array<ArrayBuffer>;
		if (selected.storage === 's3') {
			if (!this.s3) throw new SyncError('sync.blob_storage_unavailable', 503);
			const file = this.s3
				.file(this.objectKey(workspace, object, id))
				.slice(start, end + 1);
			bytes = await readBlobRange(file.stream(), end - start + 1);
		} else {
			const info = await lstat(this.path(selected.upload));
			if (!info.isFile() || info.isSymbolicLink() || info.size !== size)
				throw new SyncError('sync.blob_storage_unavailable', 503);
			bytes = new Uint8Array(
				await Bun.file(this.path(selected.upload))
					.slice(start, end + 1)
					.arrayBuffer(),
			);
			if (bytes.length !== end - start + 1)
				throw new SyncError('sync.blob_storage_unavailable', 503);
		}
		// Bun turns Response(S3File) into a presigned redirect and rejects custom
		// response options. Read bounded ciphertext instead, outside the workspace lock.
		return this.store.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				// Revocation during an external storage read must prevent publication.
				await this.authorize(tx, actor, workspace, object, role, false);
				return new Response(bytes, {
					status: 206,
					headers: {
						'Content-Type': 'application/octet-stream',
						'Content-Length': String(bytes.length),
						'Content-Range': `bytes ${start}-${end}/${size}`,
						ETag: `"${id}"`,
					},
				});
			},
		);
	}
}

/** Reject truncated, oversized, or stalled storage responses before acknowledging a range. */
export async function readBlobRange(
	stream: ReadableStream<Uint8Array>,
	expected: number,
	timeoutMs = 25_000,
): Promise<Uint8Array<ArrayBuffer>> {
	if (!Number.isSafeInteger(expected) || expected < 1 || expected > MAX_PATCH)
		throw new SyncError('sync.invalid_blob_range', 416);
	const reader = stream.getReader();
	const bytes = new Uint8Array(expected);
	let offset = 0;
	let expired = false;
	const timer = setTimeout(() => {
		expired = true;
		void reader.cancel().catch(() => {});
	}, timeoutMs);
	try {
		while (true) {
			const { done, value } = await reader.read();
			if (expired) throw new SyncError('sync.blob_storage_unavailable', 503);
			if (done) break;
			if (offset + value.length > expected)
				throw new SyncError('sync.blob_storage_unavailable', 503);
			bytes.set(value, offset);
			offset += value.length;
		}
		if (offset !== expected)
			throw new SyncError('sync.blob_storage_unavailable', 503);
		return bytes;
	} finally {
		clearTimeout(timer);
		void reader.cancel().catch(() => {});
	}
}
