import { describe, expect, test } from 'bun:test';
import { createHash, randomBytes } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BlobService } from './blobs';
import { createApp } from './app';
import { digest } from './protocol';
import { fixture } from './protocol.test';
import { SyncStore } from './store';
import { testS3Client } from './test-storage';

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'resumable ciphertext blobs',
	() => {
		test('tus resumes durable bytes, completion checks the digest, and reads enforce object access', async () => {
			const directory = await mkdtemp(join(tmpdir(), 'noura-blobs-'));
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			const s3 = testS3Client();
			const workspace = `w_${crypto.randomUUID()}`;
			const object = `o_${crypto.randomUUID()}`;
			const f = fixture(`d_${crypto.randomUUID()}`, workspace, object);
			const token = randomBytes(32).toString('base64url');
			const account = `a_${crypto.randomUUID()}`;
			try {
				await store.migrate();
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${f.make().deviceId},${account},${f.publicKey})`;
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${f.make().deviceId},now()+interval '1 hour')`;
				const actor = await store.authenticate(token);
				await store.createWorkspace(actor, workspace);
				await store.createObject(actor, workspace, object);
				let app = createApp(store, {
					origin: 'http://localhost:1900',
					blobs: await BlobService.open(store, directory, s3),
				});
				const bytes = randomBytes(2 * 1024 * 1024 + 123);
				const id = createHash('sha256').update(bytes).digest('hex');
				const path = `/v1/workspaces/${workspace}/objects/${object}/blobs`;
				const auth = {
					Authorization: `Bearer ${token}`,
					'Tus-Resumable': '1.0.0',
				};
				const create = (blob = id, size = bytes.length) =>
					app.request(path, {
						method: 'POST',
						headers: { ...auth, 'Content-Type': 'application/json' },
						body: JSON.stringify({ id: blob, size, epoch: 1 }),
					});
				const patch = (offset: number, body: Uint8Array, blob = id) =>
					app.request(`${path}/${blob}`, {
						method: 'PATCH',
						headers: {
							...auth,
							'Content-Type': 'application/offset+octet-stream',
							'Upload-Offset': String(offset),
						},
						body: new Uint8Array(body),
					});
				expect((await create()).status).toBe(201);
				expect((await create()).status).toBe(201);
				const first = await patch(0, bytes.subarray(0, 1024 * 1024));
				expect(first.status).toBe(204);
				expect(first.headers.get('Upload-Offset')).toBe(String(1024 * 1024));
				const [row] =
					await store.db`SELECT * FROM noura_blobs WHERE workspace_id=${workspace} AND id=${id}`;
				expect((await readFile(join(directory, row!.upload_id))).length).toBe(
					1024 * 1024,
				);
				expect((await patch(0, bytes.subarray(0, 32))).status).toBe(409);
				app = createApp(store, {
					origin: 'http://localhost:1900',
					blobs: await BlobService.open(store, directory, s3),
				});
				const resumed = await app.request(`${path}/${id}`, {
					method: 'HEAD',
					headers: auth,
				});
				expect(resumed.status).toBe(200);
				expect(resumed.headers.get('Upload-Offset')).toBe(String(1024 * 1024));
				expect(
					(
						await patch(
							1024 * 1024,
							bytes.subarray(1024 * 1024, 2 * 1024 * 1024),
						)
					).status,
				).toBe(204);
				const finished = await patch(
					2 * 1024 * 1024,
					bytes.subarray(2 * 1024 * 1024),
				);
				expect(finished.status).toBe(204);
				expect(finished.headers.get('Noura-Blob-Complete')).toBe('true');
				const [stored] =
					await store.db`SELECT storage FROM noura_blobs WHERE workspace_id=${workspace} AND id=${id}`;
				expect(stored!.storage).toBe(s3 ? 's3' : 'local');
				if (s3) {
					// A successful range read must come from S3, not the local upload cache.
					await rm(join(directory, row!.upload_id));
				}
				expect((await patch(bytes.length, new Uint8Array())).status).toBe(409);
				const download = await app.request(`${path}/${id}/content`, {
					headers: { ...auth, Range: 'bytes=1048570-1048600' },
				});
				expect(download.status).toBe(206);
				expect(new Uint8Array(await download.arrayBuffer())).toEqual(
					new Uint8Array(bytes.subarray(1048570, 1048601)),
				);
				const readDevice = fixture(
					`d_${crypto.randomUUID()}`,
					workspace,
					object,
				);
				const readToken = randomBytes(32).toString('base64url');
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${readDevice.make().deviceId},${account},${readDevice.publicKey})`;
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(readToken)},${readDevice.make().deviceId},now()+interval '1 hour')`;
				const readEntered = Promise.withResolvers<void>();
				const readRelease = Promise.withResolvers<void>();
				const delayedRead = {
					file: () => ({
						slice: () => ({
							stream: () =>
								new ReadableStream<Uint8Array>({
									async start(controller) {
										readEntered.resolve();
										await readRelease.promise;
										controller.enqueue(new Uint8Array(bytes.subarray(0, 11)));
										controller.close();
									},
								}),
						}),
					}),
				} as unknown as Bun.S3Client;
				await store.db`UPDATE noura_blobs SET storage='s3' WHERE workspace_id=${workspace} AND id=${id}`;
				const readApp = createApp(store, {
					origin: 'http://localhost:1900',
					blobs: await BlobService.open(store, directory, delayedRead),
				});
				const reading = readApp.request(`${path}/${id}/content`, {
					headers: {
						Authorization: `Bearer ${readToken}`,
						Range: 'bytes=0-10',
					},
				});
				await readEntered.promise;
				let readTimeout: ReturnType<typeof setTimeout> | undefined;
				try {
					await Promise.race([
						store.createObject(actor, workspace, `o_${crypto.randomUUID()}`),
						new Promise((_, reject) => {
							readTimeout = setTimeout(
								() => reject(new Error('Storage read held the workspace lock')),
								2_000,
							);
						}),
					]);
					await store.db`UPDATE noura_devices SET revoked=true WHERE id=${readDevice.make().deviceId}`;
				} finally {
					clearTimeout(readTimeout);
					readRelease.resolve();
				}
				expect((await reading).status).toBe(401);
				await store.db`UPDATE noura_blobs SET storage=${s3 ? 's3' : 'local'} WHERE workspace_id=${workspace} AND id=${id}`;
				expect(
					(
						await app.request(`${path}/${id}/content`, {
							headers: { Range: 'bytes=0-10' },
						})
					).status,
				).toBe(401);
				expect(
					(
						await app.request(`${path}/${id}/content`, {
							headers: { ...auth, Range: 'bytes=0-2000000' },
						})
					).status,
				).toBe(416);
				const wrong = '0'.repeat(64);
				expect((await create(wrong, 32)).status).toBe(201);
				expect((await patch(0, randomBytes(32), wrong)).status).toBe(409);
				expect(
					(
						await app.request(`${path}/${wrong}/content`, {
							headers: { ...auth, Range: 'bytes=0-10' },
						})
					).status,
				).toBe(404);
				expect(
					(
						await app.request(`${path}/${wrong}`, {
							method: 'DELETE',
							headers: auth,
						})
					).status,
				).toBe(204);
				expect(
					(
						await app.request(`${path}/${id}`, {
							method: 'DELETE',
							headers: auth,
						})
					).status,
				).toBe(409);
				const entered = Promise.withResolvers<void>();
				const release = Promise.withResolvers<void>();
				const slowStorage = {
					write: async () => {
						entered.resolve();
						await release.promise;
					},
				} as unknown as Bun.S3Client;
				app = createApp(store, {
					origin: 'http://localhost:1900',
					blobs: await BlobService.open(store, directory, slowStorage),
				});
				const pendingBytes = randomBytes(32);
				const pendingId = createHash('sha256')
					.update(pendingBytes)
					.digest('hex');
				expect((await create(pendingId, pendingBytes.length)).status).toBe(201);
				const completing = patch(0, pendingBytes, pendingId);
				await entered.promise;
				let timeout: ReturnType<typeof setTimeout> | undefined;
				try {
					// Object metadata remains writable while external storage is still blocked.
					await Promise.race([
						store.createObject(
							actor,
							workspace,
							`parallel_${crypto.randomUUID()}`,
						),
						new Promise((_, reject) => {
							timeout = setTimeout(
								() =>
									reject(
										new Error('Blob finalization held the workspace lock'),
									),
								2000,
							);
						}),
					]);
					await store.db`UPDATE noura_devices SET revoked=true WHERE id=${actor.deviceId}`;
				} finally {
					clearTimeout(timeout);
					release.resolve();
				}
				expect((await completing).status).toBe(401);
				const [unfinished] =
					await store.db`SELECT complete FROM noura_blobs WHERE workspace_id=${workspace} AND id=${pendingId}`;
				expect(unfinished!.complete).toBe(false);
				expect(
					(
						await app.request(`${path}/${id}/content`, {
							headers: { ...auth, Range: 'bytes=0-10' },
						})
					).status,
				).toBe(401);
			} finally {
				await store.close();
				await rm(directory, { recursive: true, force: true });
			}
		}, 60_000);
	},
);
