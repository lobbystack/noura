/** Offline local-storage restore drill. Creates and drops only its own isolated test databases. */
import assert from 'node:assert/strict';
import {
	createHash,
	generateKeyPairSync,
	randomBytes,
	sign,
} from 'node:crypto';
import { cp, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import postgres from 'postgres';
import { createApp } from '../src/app';
import { BlobService } from '../src/blobs';
import { digest, signingBytes } from '../src/protocol';
import { SyncStore } from '../src/store';
import { testS3Client } from '../src/test-storage';

const adminUrl = process.env.NOURA_TEST_DATABASE_URL;
if (!adminUrl)
	throw new Error(
		'NOURA_TEST_DATABASE_URL is required (test PostgreSQL with CREATEDB)',
	);
const root = await mkdtemp(join(tmpdir(), 'noura-restore-'));
const admin = postgres(adminUrl, { max: 1 });
const suffix = crypto.randomUUID().replaceAll('-', '');
const names = [`noura_drill_${suffix}`, `noura_restored_${suffix}`];
const created: string[] = [];
const stores: SyncStore[] = [];
const s3 = testS3Client();
const s3Keys: string[] = [];
const databaseUrl = (name: string) => {
	const url = new URL(adminUrl);
	url.pathname = `/${name}`;
	return url.toString();
};
async function pg(tool: string, database: string, args: string[]) {
	const url = new URL(adminUrl!);
	const command = process.env.NOURA_TEST_PG_BIN
		? join(process.env.NOURA_TEST_PG_BIN, tool)
		: tool;
	const child = Bun.spawn([command, ...args], {
		env: {
			...process.env,
			PGHOST: url.hostname,
			PGPORT: url.port || '5432',
			PGUSER: decodeURIComponent(url.username),
			PGPASSWORD: decodeURIComponent(url.password),
			PGDATABASE: database,
		},
		stdout: 'ignore',
		stderr: 'pipe',
	});
	const [code] = await Promise.all([
		child.exited,
		new Response(child.stderr).text(),
	]);
	assert.equal(
		code,
		0,
		`${tool} failed; check the test PostgreSQL configuration`,
	);
}
try {
	for (const name of names) {
		await admin`CREATE DATABASE ${admin(name)}`;
		created.push(name);
	}
	const source = new SyncStore(databaseUrl(names[0]!));
	stores.push(source);
	await source.migrate();
	const workspace = `w_${suffix}`,
		object = `o_${suffix}`,
		device = `d_${suffix}`;
	const token = randomBytes(32).toString('base64url');
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ type: 'spki', format: 'der' })
		.subarray(-32)
		.toString('base64');
	await source.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${device},${`a_${suffix}`},${publicKey})`;
	await source.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${device},now()+interval '1 hour')`;
	const actor = await source.authenticate(token);
	await source.createWorkspace(actor, workspace);
	await source.createObject(actor, workspace, object);
	let app = createApp(source, {
		origin: 'http://localhost:1900',
		blobs: await BlobService.open(source, join(root, 'source-blobs'), s3),
	});
	const path = `/v1/workspaces/${workspace}/objects/${object}/blobs`;
	const headers = {
		Authorization: `Bearer ${token}`,
		'Tus-Resumable': '1.0.0',
	};
	const completed = randomBytes(128 * 1024 + 13),
		partial = randomBytes(192 * 1024 + 7);
	const hash = (bytes: Uint8Array) =>
		createHash('sha256').update(bytes).digest('hex');
	async function patch(id: string, offset: number, bytes: Uint8Array) {
		const response = await app.request(`${path}/${id}`, {
			method: 'PATCH',
			headers: {
				...headers,
				'Content-Type': 'application/offset+octet-stream',
				'Upload-Offset': String(offset),
			},
			body: new Uint8Array(bytes),
		});
		if (response.status !== 204) {
			const failure = (await response
				.clone()
				.json()
				.catch(() => null)) as {
				error?: { code?: string };
			} | null;
			assert.equal(
				response.status,
				204,
				failure?.error?.code ?? 'blob PATCH failed without an error code',
			);
		}
		return response;
	}
	for (const bytes of [completed, partial]) {
		const response = await app.request(path, {
			method: 'POST',
			headers: { ...headers, 'Content-Type': 'application/json' },
			body: JSON.stringify({ id: hash(bytes), size: bytes.length, epoch: 1 }),
		});
		assert.equal(response.status, 201);
	}
	assert.equal(
		(await patch(hash(completed), 0, completed)).headers.get(
			'Noura-Blob-Complete',
		),
		'true',
	);
	await patch(hash(partial), 0, partial.subarray(0, 65537));
	const body = {
		version: 1 as const,
		workspaceId: workspace,
		objectId: object,
		deviceId: device,
		operationId: `op_${suffix}`,
		epoch: 1,
		policyRevision: '0',
		nonce: randomBytes(12).toString('base64'),
		ciphertext: randomBytes(64).toString('base64'),
	};
	const operation = {
		...body,
		signature: sign(null, signingBytes(body), keys.privateKey).toString(
			'base64',
		),
	};
	await source.push(actor, workspace, [operation]);
	const before = await source.pull(actor, workspace, '0');
	const [quota] =
		await source.db`SELECT used_bytes FROM noura_workspaces WHERE id=${workspace}`;
	const [completedRow] =
		await source.db`SELECT upload_id FROM noura_blobs WHERE workspace_id=${workspace} AND id=${hash(completed)}`;
	if (s3)
		s3Keys.push(
			...[completed, partial].map(
				(bytes) => `blobs/${workspace}/${object}/${hash(bytes)}`,
			),
		);
	// All requests have finished; closing the only writer gives a coherent offline boundary.
	await source.close();
	await mkdir(join(root, 'backup'), { mode: 0o700 });
	await pg('pg_dump', names[0]!, [
		'--format=custom',
		'--no-owner',
		'--no-acl',
		'--file',
		join(root, 'backup', 'database.dump'),
	]);
	await cp(join(root, 'source-blobs'), join(root, 'backup', 'blobs'), {
		recursive: true,
	});
	await rm(join(root, 'source-blobs'), { recursive: true });
	if (s3) {
		const saved = join(root, 'backup', 'completed.s3');
		await Bun.write(saved, await s3.file(s3Keys[0]!).arrayBuffer());
		assert.equal(
			hash(new Uint8Array(await Bun.file(saved).arrayBuffer())),
			hash(completed),
		);
		await s3.delete(s3Keys[0]!);
		assert.equal(await s3.file(s3Keys[0]!).exists(), false);
		await s3.write(s3Keys[0]!, Bun.file(saved));
	}
	await pg('pg_restore', names[1]!, [
		'--exit-on-error',
		'--no-owner',
		'--no-acl',
		'--dbname',
		names[1]!,
		join(root, 'backup', 'database.dump'),
	]);
	await cp(join(root, 'backup', 'blobs'), join(root, 'restored-blobs'), {
		recursive: true,
	});
	if (s3) await rm(join(root, 'restored-blobs', completedRow!.upload_id));
	const restored = new SyncStore(databaseUrl(names[1]!));
	stores.push(restored);
	app = createApp(restored, {
		origin: 'http://localhost:1900',
		blobs: await BlobService.open(restored, join(root, 'restored-blobs'), s3),
	});
	const restoredActor = await restored.authenticate(token);
	assert.deepEqual(await restored.pull(restoredActor, workspace, '0'), before);
	const [restoredQuota] =
		await restored.db`SELECT used_bytes FROM noura_workspaces WHERE id=${workspace}`;
	assert.equal(restoredQuota!.used_bytes, quota!.used_bytes);
	const head = await app.request(`${path}/${hash(partial)}`, {
		method: 'HEAD',
		headers,
	});
	assert.equal(head.status, 200);
	assert.equal(head.headers.get('Upload-Offset'), '65537');
	assert.equal(
		(await patch(hash(partial), 65537, partial.subarray(65537))).headers.get(
			'Noura-Blob-Complete',
		),
		'true',
	);
	for (const bytes of [completed, partial]) {
		const response = await app.request(`${path}/${hash(bytes)}/content`, {
			headers: { ...headers, Range: `bytes=0-${bytes.length - 1}` },
		});
		assert.equal(response.status, 206);
		assert.equal(
			hash(new Uint8Array(await response.arrayBuffer())),
			hash(bytes),
		);
	}
	assert.deepEqual(await restored.push(restoredActor, workspace, [operation]), [
		'1',
	]);
	console.log(
		`Restore drill passed (${s3 ? 'S3 plus local staging' : 'local storage'}): sessions, signed operations, cursors, quotas, completed ciphertext, and resumed partial uploads.`,
	);
} finally {
	const outcomes = await Promise.allSettled([
		...s3Keys.map((key) => s3!.delete(key)),
		...stores.map((store) => store.close()),
	]);
	outcomes.push(
		...(await Promise.allSettled(
			created.map(async (name) => {
				await admin`DROP DATABASE ${admin(name)} WITH (FORCE)`;
			}),
		)),
	);
	outcomes.push(
		...(await Promise.allSettled([
			admin.end(),
			rm(root, { recursive: true, force: true }),
		])),
	);
	const failures = outcomes.filter((outcome) => outcome.status === 'rejected');
	if (failures.length)
		throw new AggregateError(
			failures.map((outcome) => outcome.reason),
			'Restore drill cleanup failed',
		);
}
