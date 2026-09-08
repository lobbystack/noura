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
import {
	accessDigest,
	accessSigningBytes,
	keySigningBytes,
	setAccess,
	type AccessPolicy,
} from '../src/access';
import {
	capabilitySigningBytes,
	putWorkspaceCapability,
	type WorkspaceCapability,
} from '../src/capabilities';
import {
	checkpointSigningBytes,
	stageTransition,
	transitionSigningBytes,
	type AccessTransition,
	type EncryptedCheckpoint,
} from '../src/checkpoints';

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
	const capability: WorkspaceCapability = {
		version: 1,
		workspaceId: workspace,
		collaborationVersion: 1,
		minimumClientVersion: 1,
		minimumRelayVersion: 1,
		deviceId: device,
		signature: '',
	};
	capability.signature = sign(
		null,
		capabilitySigningBytes(capability),
		keys.privateKey,
	).toString('base64');
	await putWorkspaceCapability(source, actor, capability);
	const transition = (
		revision: string,
		epoch: number,
		previousPolicyDigest: string | null,
		blobs: AccessTransition['blobs'] = [],
	): AccessTransition => {
		const generation = `generation_${revision}_${suffix}`;
		const envelope = {
			deviceId: device,
			wrappedKey: randomBytes(80).toString('base64'),
			signature: '',
		};
		envelope.signature = sign(
			null,
			keySigningBytes(workspace, object, epoch, device, envelope),
			keys.privateKey,
		).toString('base64');
		const policy: AccessPolicy = {
			version: 2,
			workspaceId: workspace,
			revision,
			previousPolicyDigest,
			deviceId: device,
			members: [{ accountId: actor.accountId, role: 'owner' }],
			objects: [
				{
					objectId: object,
					epoch,
					grants: [],
					envelopes: [envelope],
					document: { generation, mode: 'text' },
				},
			],
			signature: '',
		};
		policy.signature = sign(
			null,
			accessSigningBytes(policy),
			keys.privateKey,
		).toString('base64');
		const payloadBody = {
			version: 1 as const,
			workspaceId: workspace,
			objectId: object,
			deviceId: device,
			operationId: `checkpoint_${revision}_${suffix}`,
			epoch,
			policyRevision: revision,
			nonce: randomBytes(12).toString('base64'),
			ciphertext: randomBytes(64).toString('base64'),
		};
		const checkpoint: EncryptedCheckpoint = {
			version: 1,
			generation,
			coveredSequence: '1',
			payload: {
				...payloadBody,
				signature: sign(
					null,
					signingBytes(payloadBody),
					keys.privateKey,
				).toString('base64'),
			},
			signature: '',
		};
		checkpoint.signature = sign(
			null,
			checkpointSigningBytes(checkpoint),
			keys.privateKey,
		).toString('base64');
		const value: AccessTransition = {
			version: blobs.length ? 2 : 1,
			transitionId: `transition_${revision}_${suffix}`,
			coveredSequence: '1',
			policy,
			checkpoints: [checkpoint],
			...(blobs.length ? { blobs } : {}),
			signature: '',
		};
		value.signature = sign(
			null,
			transitionSigningBytes(value),
			keys.privateKey,
		).toString('base64');
		return value;
	};
	const committedTransition = transition('1', 2, null);
	await stageTransition(source, actor, committedTransition);
	await setAccess(
		source,
		actor,
		committedTransition.policy,
		committedTransition,
	);
	for (const bytes of [completed, partial]) {
		assert.equal(
			(
				await app.request(path, {
					method: 'POST',
					headers: { ...headers, 'Content-Type': 'application/json' },
					body: JSON.stringify({
						id: hash(bytes),
						size: bytes.length,
						epoch: 2,
					}),
				})
			).status,
			201,
		);
	}
	assert.equal(
		(await patch(hash(completed), 0, completed)).headers.get(
			'Noura-Blob-Complete',
		),
		'true',
	);
	await patch(hash(partial), 0, partial.subarray(0, 65537));
	const stagedTransition = transition(
		'2',
		3,
		accessDigest(committedTransition.policy),
		[
			{
				objectId: object,
				epoch: 3,
				ciphertextDigest: 'a'.repeat(64),
				ciphertextSize: 1024,
			},
		],
	);
	await stageTransition(source, actor, stagedTransition);
	const before = await source.pull(actor, workspace, '0');
	const transitionRows =
		await source.db`SELECT id,digest,body,committed FROM noura_transitions WHERE workspace_id=${workspace} ORDER BY id`;
	const checkpointRows =
		await source.db`SELECT object_id,epoch,generation,checkpoint FROM noura_checkpoints WHERE workspace_id=${workspace}`;
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
	assert.deepEqual(
		await restored.db`SELECT id,digest,body,committed FROM noura_transitions WHERE workspace_id=${workspace} ORDER BY id`,
		transitionRows,
	);
	assert.deepEqual(
		await restored.db`SELECT object_id,epoch,generation,checkpoint FROM noura_checkpoints WHERE workspace_id=${workspace}`,
		checkpointRows,
	);
	assert.equal(transitionRows.filter((row) => row.committed).length, 1);
	assert.equal(transitionRows.filter((row) => !row.committed).length, 1);
	assert.equal(checkpointRows.length, 1);
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
		`Restore drill passed (${s3 ? 'S3 plus local staging' : 'local storage'}): sessions, signed operations, cursors, quotas, completed ciphertext, resumed partial uploads, staged/committed transitions, checkpoint generations, and blob manifests.`,
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
