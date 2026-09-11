/**
 * Experimental realtime collaboration delivery probe.
 *
 * Defaults to one 20-writer burst across 100 connected clients with 100 ms
 * simulated RTT. Set NOURA_SOAK_SECONDS=3600 for the release soak. Shared CI
 * runners raise NOURA_LOAD_P95_BUDGET_MS above the 1000 ms default. This probes
 * signed version-two operation delivery through WebSocket invalidation followed
 * by authoritative HTTP pull; native CRDT application remains a desktop gate.
 */
import assert from 'node:assert/strict';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { websocket } from 'hono/bun';
import type { EncryptedOperation } from '../../../packages/shared/src/sync';
import { createApp } from '../src/app';
import { digest, signingBytes } from '../src/protocol';
import { SyncStore } from '../src/store';

function integer(
	name: string,
	fallback: number,
	minimum: number,
	maximum: number,
) {
	const source = process.env[name];
	const value = source === undefined ? fallback : Number(source);
	if (!Number.isSafeInteger(value) || value < minimum || value > maximum)
		throw new Error(
			`${name} must be an integer from ${minimum} through ${maximum}`,
		);
	return value;
}

const database = process.env.NOURA_TEST_DATABASE_URL;
if (!database) throw new Error('NOURA_TEST_DATABASE_URL is required');
const clientCount = integer('NOURA_LOAD_CLIENTS', 100, 2, 500);
const writerCount = integer('NOURA_LOAD_WRITERS', 20, 1, clientCount);
const simulatedRttMs = integer('NOURA_LOAD_RTT_MS', 100, 0, 10_000);
const soakSeconds = integer('NOURA_SOAK_SECONDS', 0, 0, 86_400);
const intervalMs = integer('NOURA_LOAD_INTERVAL_MS', 1_000, 100, 60_000);
const maxRssGrowthMiB = integer('NOURA_MAX_RSS_GROWTH_MIB', 256, 1, 16_384);
const p95BudgetMs = integer('NOURA_LOAD_P95_BUDGET_MS', 1_000, 1, 60_000);
const forceReconnectBatch = integer(
	'NOURA_LOAD_FORCE_RECONNECT_BATCH',
	0,
	0,
	86_400,
);
const forceRecoveryBatch = integer(
	'NOURA_LOAD_FORCE_RECOVERY_BATCH',
	0,
	0,
	86_400,
);
const halfRttMs = Math.ceil(simulatedRttMs / 2);
const sleep = (milliseconds: number) =>
	new Promise<void>((resolve) => setTimeout(resolve, milliseconds));
// Bun supports custom client handshake headers; the ambient DOM overload omits
// this runtime-specific constructor form.
type ProbeWebSocket = WebSocket & { terminate(): void };
const AuthenticatedWebSocket = WebSocket as unknown as new (
	url: string,
	options: { headers: Record<string, string> },
) => ProbeWebSocket;

class LatencyHistogram {
	private readonly milliseconds = new Uint32Array(10_001);
	count = 0;
	maximum = 0;
	add(value: number) {
		const rounded = Math.max(0, Math.ceil(value));
		this.milliseconds[Math.min(rounded, this.milliseconds.length - 1)]! += 1;
		this.maximum = Math.max(this.maximum, rounded);
		this.count += 1;
	}
	percentile(fraction: number) {
		const target = Math.max(1, Math.ceil(this.count * fraction));
		let seen = 0;
		for (let index = 0; index < this.milliseconds.length; index += 1) {
			seen += this.milliseconds[index]!;
			if (seen >= target) return index;
		}
		return this.milliseconds.length - 1;
	}
}

const store = new SyncStore(database);
const runId = crypto.randomUUID().replaceAll('-', '');
const workspace = `collab_load_${runId}`;
const object = `object_${runId}`;
const generation = `generation_${runId}`;
const keys = generateKeyPairSync('ed25519');
const publicKey = keys.publicKey
	.export({ format: 'der', type: 'spki' })
	.subarray(-32)
	.toString('base64');
const owner = {
	accountId: `owner_${runId}`,
	deviceId: `owner_${runId}`,
	publicKey,
};
const clients = Array.from({ length: clientCount }, (_, index) => ({
	accountId: `account_${index}_${runId}`,
	deviceId: `device_${index}_${runId}`,
	token: randomBytes(32).toString('base64url'),
	cursor: '0',
	received: new Set<string>(),
	pull: Promise.resolve(),
	socket: undefined as ProbeWebSocket | undefined,
	connecting: undefined as Promise<void> | undefined,
	reconnects: 0,
}));
const histogram = new LatencyHistogram();
const activeStarted = new Map<string, number>();
const activeDeliveries = new Map<string, number>();
let expectedBatchDeliveries = 0;
let batchDeliveries = 0;
let finishBatch: (() => void) | undefined;
let failure: unknown;
let server: ReturnType<typeof Bun.serve> | undefined;
let baselineRss = 0;
let peakRss = 0;
let batches = 0;
let shuttingDown = false;
let recoveryPulls = 0;

function fail(error: unknown) {
	failure ??= error;
	finishBatch?.();
}

async function pull(index: number) {
	const client = clients[index]!;
	if (halfRttMs) await sleep(halfRttMs);
	const response = await fetch(
		`http://127.0.0.1:${server!.port}/v1/workspaces/${workspace}/operations?after=${client.cursor}`,
		{ headers: { Authorization: `Bearer ${client.token}` } },
	);
	if (!response.ok)
		throw new Error(
			`client ${index} pull failed: ${response.status} ${await response.text()}`,
		);
	const page = (await response.json()) as {
		cursor: string;
		operations: Array<{ operationId: string }>;
	};
	for (const operation of page.operations) {
		const started = activeStarted.get(operation.operationId);
		if (started === undefined)
			throw new Error(
				`client ${index} received an operation outside the active batch`,
			);
		if (client.received.has(operation.operationId))
			throw new Error(`client ${index} received a duplicate operation`);
		client.received.add(operation.operationId);
		histogram.add(performance.now() - started);
		activeDeliveries.set(
			operation.operationId,
			(activeDeliveries.get(operation.operationId) ?? 0) + 1,
		);
		batchDeliveries += 1;
	}
	client.cursor = page.cursor;
	if (batchDeliveries === expectedBatchDeliveries) finishBatch?.();
}

function schedulePull(index: number) {
	const client = clients[index]!;
	client.pull = client.pull.then(() => pull(index)).catch(fail);
}

function connectClient(index: number): Promise<void> {
	const client = clients[index]!;
	if (client.socket?.readyState === WebSocket.OPEN) return Promise.resolve();
	if (client.connecting) return client.connecting;
	let opened = false;
	let tracked!: Promise<void>;
	const attempt = new Promise<void>((resolve, reject) => {
		const socket = new AuthenticatedWebSocket(
			`ws://127.0.0.1:${server!.port}/v1/workspaces/${workspace}/realtime`,
			{ headers: { Authorization: `Bearer ${client.token}` } },
		);
		client.socket = socket;
		const opening = setTimeout(() => {
			socket.terminate();
			reject(new Error(`client ${index} did not receive hello`));
		}, 10_000);
		socket.onmessage = (event) => {
			try {
				const message = JSON.parse(String(event.data)) as { type?: string };
				if (message.type === 'hello') {
					opened = true;
					clearTimeout(opening);
					resolve();
				} else if (
					message.type === 'changed' &&
					!(forceRecoveryBatch === batches + 1 && index === clientCount - 2)
				) {
					schedulePull(index);
				}
			} catch (error) {
				fail(error);
			}
		};
		socket.onerror = () => {
			// The close callback either starts bounded recovery or rejects startup.
		};
		socket.onclose = () => {
			clearTimeout(opening);
			if (client.socket === socket) client.socket = undefined;
			if (shuttingDown) return;
			if (!opened) {
				reject(new Error(`client ${index} WebSocket failed before hello`));
				return;
			}
			client.reconnects += 1;
			// WebSockets are invalidations only. Recover the active cursor through
			// the authoritative path before waiting for another notification.
			schedulePull(index);
			setTimeout(() => void connectClient(index).catch(fail), 1_000);
		};
	});
	tracked = attempt.finally(() => {
		if (client.connecting === tracked) client.connecting = undefined;
	});
	client.connecting = tracked;
	return tracked;
}

async function waitFor(
	predicate: () => boolean,
	milliseconds: number,
	message: string,
) {
	const deadline = performance.now() + milliseconds;
	while (!predicate()) {
		if (failure) throw failure;
		if (performance.now() >= deadline) throw new Error(message);
		await sleep(10);
	}
}

function operation(deviceId: string): EncryptedOperation {
	const value: EncryptedOperation = {
		version: 2,
		workspaceId: workspace,
		objectId: object,
		deviceId,
		operationId: `operation_${crypto.randomUUID()}`,
		epoch: 1,
		policyRevision: '0',
		generation,
		kind: 'text',
		nonce: randomBytes(12).toString('base64'),
		ciphertext: randomBytes(256).toString('base64'),
		signature: '',
	};
	value.signature = sign(null, signingBytes(value), keys.privateKey).toString(
		'base64',
	);
	return value;
}

async function runBatch() {
	await Promise.all(clients.map((_client, index) => connectClient(index)));
	for (const client of clients) client.received.clear();
	activeStarted.clear();
	activeDeliveries.clear();
	batchDeliveries = 0;
	expectedBatchDeliveries = clientCount * writerCount;
	const done = new Promise<void>((resolve) => {
		finishBatch = resolve;
	});
	const operations = clients.slice(0, writerCount).map((client) => {
		const value = operation(client.deviceId);
		activeStarted.set(value.operationId, performance.now());
		activeDeliveries.set(value.operationId, 0);
		return { client, value };
	});
	if (halfRttMs) await sleep(halfRttMs);
	await Promise.all(
		operations.map(async ({ client, value }) => {
			const response = await fetch(
				`http://127.0.0.1:${server!.port}/v1/workspaces/${workspace}/operations`,
				{
					method: 'POST',
					headers: {
						Authorization: `Bearer ${client.token}`,
						'Content-Type': 'application/json',
					},
					body: JSON.stringify({ operations: [value] }),
				},
			);
			if (!response.ok)
				throw new Error(
					`writer push failed: ${response.status} ${await response.text()}`,
				);
		}),
	);
	if (forceReconnectBatch === batches + 1) clients.at(-1)!.socket?.terminate();
	let timeout: ReturnType<typeof setTimeout> | undefined;
	let recovery: ReturnType<typeof setTimeout> | undefined;
	try {
		recovery = setTimeout(() => {
			for (const [index, client] of clients.entries()) {
				if (client.received.size === writerCount) continue;
				recoveryPulls += 1;
				schedulePull(index);
			}
		}, 5_000);
		await Promise.race([
			done,
			new Promise<never>((_resolve, reject) => {
				timeout = setTimeout(
					() => {
						const missingClients = clients
							.map((client, index) => ({
								index,
								received: client.received.size,
							}))
							.filter((client) => client.received !== writerCount)
							.slice(0, 10);
						const missingOperations = [...activeDeliveries]
							.filter(([, count]) => count !== clientCount)
							.slice(0, 10);
						reject(
							new Error(
								`realtime batch delivery timed out: ${JSON.stringify({ batchDeliveries, expectedBatchDeliveries, missingClients, missingOperations, subscriptions: store.subscriptionCount })}`,
							),
						);
					},
					Math.max(30_000, simulatedRttMs * 10),
				);
			}),
		]);
	} finally {
		if (timeout) clearTimeout(timeout);
		if (recovery) clearTimeout(recovery);
	}
	await Promise.all(clients.map((client) => client.pull));
	if (failure) throw failure;
	for (const client of clients) client.pull = Promise.resolve();
	assert.equal(batchDeliveries, expectedBatchDeliveries);
	for (const count of activeDeliveries.values())
		assert.equal(count, clientCount);
	batches += 1;
	peakRss = Math.max(peakRss, process.memoryUsage().rss);
}

try {
	await store.migrate();
	await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${owner.deviceId},${owner.accountId},${owner.publicKey})`;
	await store.createWorkspace(owner, workspace);
	await store.createObject(owner, workspace, object);
	await store.db`UPDATE noura_objects SET generation=${generation},document_mode='text' WHERE workspace_id=${workspace} AND id=${object}`;
	await store.transaction(async (tx) => {
		for (const [index, client] of clients.entries()) {
			await tx`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${client.deviceId},${client.accountId},${publicKey})`;
			await tx`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(client.token)},${client.deviceId},now()+interval '2 hours')`;
			await tx`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${client.accountId},${index < writerCount ? 'editor' : 'viewer'})`;
		}
	});
	const app = createApp(store, {
		origin: 'http://127.0.0.1:1900',
		realtime: true,
		onInternalError: (error) => console.error(error),
	});
	server = Bun.serve({
		hostname: '127.0.0.1',
		port: 0,
		fetch: app.fetch,
		maxRequestBodySize: 2 * 1024 * 1024,
		idleTimeout: 30,
		websocket: {
			...websocket,
			maxPayloadLength: 16 * 1024,
			backpressureLimit: 1024 * 1024,
			closeOnBackpressureLimit: true,
			idleTimeout: 35,
		},
	});
	await Promise.all(clients.map((_client, index) => connectClient(index)));
	await waitFor(
		() => store.subscriptionCount === clientCount * 2,
		10_000,
		'realtime subscriptions did not stabilize',
	);
	baselineRss = process.memoryUsage().rss;
	peakRss = baselineRss;
	const stopAt = performance.now() + soakSeconds * 1_000;
	do {
		const batchStarted = performance.now();
		await runBatch();
		if (soakSeconds && batches % 300 === 0)
			console.info(
				JSON.stringify({
					progress: true,
					batches,
					deliveries: histogram.count,
				}),
			);
		const remaining = intervalMs - (performance.now() - batchStarted);
		if (soakSeconds && remaining > 0) await sleep(remaining);
	} while (soakSeconds && performance.now() < stopAt);
	const p95Ms = histogram.percentile(0.95);
	const rssGrowthMiB = Math.max(0, peakRss - baselineRss) / 1024 / 1024;
	const result = {
		clients: clientCount,
		writers: writerCount,
		simulatedRttMs,
		soakSeconds,
		batches,
		deliveries: histogram.count,
		p50Ms: histogram.percentile(0.5),
		p95Ms,
		p95BudgetMs,
		maxMs: histogram.maximum,
		rssGrowthMiB: Number(rssGrowthMiB.toFixed(1)),
		reconnects: clients.reduce((total, client) => total + client.reconnects, 0),
		recoveryPulls,
	};
	console.info(JSON.stringify(result));
	if (p95Ms >= p95BudgetMs)
		throw new Error(
			'collaboration delivery p95 exceeded the configured budget',
		);
	if (rssGrowthMiB > maxRssGrowthMiB)
		throw new Error(
			'collaboration process memory growth exceeded the configured bound',
		);
} finally {
	shuttingDown = true;
	finishBatch = undefined;
	for (const client of clients) client.socket?.terminate();
	await waitFor(
		() => store.subscriptionCount === 0,
		10_000,
		'realtime subscriptions leaked after disconnect',
	).catch((error) => {
		failure ??= error;
	});
	await Promise.allSettled(clients.map((client) => client.pull));
	if (server) {
		server.unref();
		void server.stop(true).catch(() => {});
	}
	await store.close();
}
if (failure) throw failure;
