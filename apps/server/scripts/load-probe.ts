import { randomBytes } from 'node:crypto';
import { createApp } from '../src/app';
import { SyncStore } from '../src/store';
import { digest } from '../src/protocol';
import { generateKeyPairSync, sign } from 'node:crypto';
import { signingBytes } from '../src/protocol';

const database = process.env.NOURA_TEST_DATABASE_URL;
if (!database) throw new Error('NOURA_TEST_DATABASE_URL is required');
const store = new SyncStore(database);
const id = crypto.randomUUID();
const workspace = `load_${id}`;
const keys = generateKeyPairSync('ed25519');
const publicKey = keys.publicKey
	.export({ format: 'der', type: 'spki' })
	.subarray(-32)
	.toString('base64');
const owner = { accountId: `owner_${id}`, deviceId: `owner_${id}`, publicKey };
const clients = Array.from({ length: 100 }, (_, index) => ({
	device: `reader_${index}_${id}`,
	token: randomBytes(32).toString('base64url'),
}));
let server: ReturnType<typeof Bun.serve> | undefined;
const cancellation = new AbortController();
const timeout = setTimeout(() => cancellation.abort(), 45_000);
try {
	await store.migrate();
	await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${owner.deviceId},${owner.accountId},${publicKey})`;
	await store.createWorkspace(owner, workspace);
	await store.createObject(owner, workspace, 'object');
	await store.transaction(async (tx) => {
		for (const client of clients) {
			await tx`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${client.device},${client.device},${publicKey})`;
			await tx`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(client.token)},${client.device},now()+interval '1 hour')`;
			await tx`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${client.device},${clients.indexOf(client) < 20 ? 'editor' : 'viewer'})`;
		}
	});
	const app = createApp(store, { origin: 'http://127.0.0.1:1900' });
	server = Bun.serve({
		hostname: '127.0.0.1',
		port: 0,
		fetch: app.fetch,
		idleTimeout: 30,
	});
	const writers = clients.slice(0, 20);
	const started = new Map<string, number>();
	const readers = clients.map(async (client) => {
		let cursor = '0';
		const received = new Set<string>();
		const timings: number[] = [];
		while (received.size < writers.length) {
			const response = await fetch(
				`http://127.0.0.1:${server!.port}/v1/workspaces/${workspace}/operations?after=${cursor}&accessRevision=0&wait=25`,
				{
					headers: { Authorization: `Bearer ${client.token}` },
					signal: cancellation.signal,
				},
			);
			if (!response.ok) throw new Error('Concurrent pull failed');
			const page = (await response.json()) as {
				cursor: string;
				operations: { operationId: string }[];
			};
			for (const operation of page.operations) {
				const sent = started.get(operation.operationId);
				if (sent === undefined || received.has(operation.operationId))
					throw new Error('Unexpected or duplicate delivery');
				received.add(operation.operationId);
				timings.push(performance.now() - sent);
			}
			cursor = page.cursor;
		}
		return timings;
	});
	// Attach rejection handling before waiting for subscription registration.
	const completed = Promise.all(readers);
	void completed.catch(() => {});
	const deadline = performance.now() + 10_000;
	while (store.subscriptionCount < clients.length) {
		if (performance.now() > deadline)
			throw new Error('Clients did not subscribe within ten seconds');
		await new Promise((resolve) => setTimeout(resolve, 10));
	}
	await Promise.all(
		writers.map(async (writer) => {
			const operation = {
				version: 1 as const,
				workspaceId: workspace,
				objectId: 'object',
				deviceId: writer.device,
				operationId: `op_${crypto.randomUUID()}`,
				epoch: 1,
				policyRevision: '0',
				nonce: randomBytes(12).toString('base64'),
				ciphertext: randomBytes(64).toString('base64'),
				signature: '',
			};
			operation.signature = sign(
				null,
				signingBytes(operation),
				keys.privateKey,
			).toString('base64');
			started.set(operation.operationId, performance.now());
			const response = await fetch(
				`http://127.0.0.1:${server!.port}/v1/workspaces/${workspace}/operations`,
				{
					method: 'POST',
					headers: {
						Authorization: `Bearer ${writer.token}`,
						'Content-Type': 'application/json',
					},
					body: JSON.stringify({ operations: [operation] }),
					signal: cancellation.signal,
				},
			);
			if (!response.ok) throw new Error('Concurrent push failed');
		}),
	);
	const timings = (await completed).flat().sort((a, b) => a - b);
	const percentile = (p: number) =>
		Math.round(timings[Math.ceil(timings.length * p) - 1]!);
	console.info(
		JSON.stringify({
			clients: clients.length,
			writers: writers.length,
			deliveries: timings.length,
			p50Ms: percentile(0.5),
			p95Ms: percentile(0.95),
			maxMs: percentile(1),
			remainingSubscriptions: store.subscriptionCount,
		}),
	);
	if (percentile(0.95) >= 1000)
		throw new Error('File relay p95 exceeded one second');
} finally {
	clearTimeout(timeout);
	cancellation.abort();
	await server?.stop(true);
	await store.close();
}
