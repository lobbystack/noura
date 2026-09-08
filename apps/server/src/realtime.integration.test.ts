import { describe, expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { websocket } from 'hono/bun';
import type { EncryptedPresence } from '../../../packages/shared/src/generated/EncryptedPresence';
import { createApp } from './app';
import { digest } from './protocol';
import { presenceSigningBytes } from './realtime';
import { SyncStore } from './store';

type ProbeSocket = WebSocket & { terminate(): void };
const AuthenticatedWebSocket = WebSocket as unknown as new (
	url: string,
	options: { headers: Record<string, string> },
) => ProbeSocket;
const sleep = (milliseconds: number) =>
	new Promise<void>((resolve) => setTimeout(resolve, milliseconds));

function keyPair() {
	const keys = generateKeyPairSync('ed25519');
	return {
		keys,
		publicKey: keys.publicKey
			.export({ type: 'spki', format: 'der' })
			.subarray(-32)
			.toString('base64'),
	};
}

function nextMessage<T>(
	socket: WebSocket,
	predicate: (value: unknown) => value is T,
	milliseconds = 5_000,
) {
	return new Promise<T>((resolve, reject) => {
		const timeout = setTimeout(() => {
			socket.removeEventListener('message', receive);
			reject(new Error('WebSocket message timed out'));
		}, milliseconds);
		const receive = (event: MessageEvent) => {
			let value: unknown;
			try {
				value = JSON.parse(String(event.data));
			} catch {
				return;
			}
			if (!predicate(value)) return;
			clearTimeout(timeout);
			socket.removeEventListener('message', receive);
			resolve(value);
		};
		socket.addEventListener('message', receive);
	});
}

function nextClose(socket: WebSocket, milliseconds = 5_000) {
	return new Promise<CloseEvent>((resolve, reject) => {
		const timeout = setTimeout(() => {
			socket.removeEventListener('close', close);
			reject(new Error('WebSocket close timed out'));
		}, milliseconds);
		const close = (event: CloseEvent) => {
			clearTimeout(timeout);
			socket.removeEventListener('close', close);
			resolve(event);
		};
		socket.addEventListener('close', close);
	});
}

function isHello(
	value: unknown,
): value is { type: 'hello'; sessionId: string } {
	return (
		!!value &&
		typeof value === 'object' &&
		(value as { type?: unknown }).type === 'hello' &&
		typeof (value as { sessionId?: unknown }).sessionId === 'string'
	);
}

async function connect(port: number, workspace: string, token: string) {
	const socket = new AuthenticatedWebSocket(
		`ws://127.0.0.1:${port}/v1/workspaces/${workspace}/realtime`,
		{ headers: { Authorization: `Bearer ${token}` } },
	);
	return { socket, hello: await nextMessage(socket, isHello) };
}

async function subscriptions(store: SyncStore, expected: number) {
	const deadline = performance.now() + 5_000;
	while (store.subscriptionCount !== expected) {
		if (performance.now() >= deadline)
			throw new Error(`expected ${expected} realtime subscriptions`);
		await sleep(10);
	}
}

function stopTestServer(server: ReturnType<typeof Bun.serve> | undefined) {
	if (!server) return;
	// Bun keeps upgraded connections in its graceful-stop promise until the
	// server idle timeout even after both WebSocket close events have fired.
	server.unref();
	void server.stop(true).catch(() => {});
}

describe.skipIf(!process.env.NOURA_TEST_DATABASE_URL)(
	'realtime collaboration sockets',
	() => {
		test('a cross-instance device revocation closes an existing subscription', async () => {
			const primary = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			const peer = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await primary.migrate();
			const id = crypto.randomUUID();
			const workspace = `workspace_${id}`;
			const deviceId = `device_${id}`;
			const accountId = `account_${id}`;
			const token = randomBytes(32).toString('base64url');
			const { publicKey } = keyPair();
			let socket: ProbeSocket | undefined;
			let server: ReturnType<typeof Bun.serve> | undefined;
			try {
				await primary.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${deviceId},${accountId},${publicKey})`;
				await primary.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${deviceId},now()+interval '1 hour')`;
				await primary.createWorkspace(
					{ deviceId, accountId, publicKey },
					workspace,
				);
				const app = createApp(primary, {
					origin: 'http://127.0.0.1:1900',
					realtime: true,
				});
				server = Bun.serve({
					hostname: '127.0.0.1',
					port: 0,
					fetch: app.fetch,
					websocket,
				});
				({ socket } = await connect(server.port!, workspace, token));
				await subscriptions(primary, 2);
				const closed = nextClose(socket);
				await peer.transaction(async (tx) => {
					await tx`UPDATE noura_devices SET revoked=true WHERE id=${deviceId}`;
					await tx`DELETE FROM noura_sessions WHERE device_id=${deviceId}`;
					await tx`SELECT pg_notify('noura_sync',${workspace})`;
				});
				expect((await closed).code).toBe(4403);
				await subscriptions(primary, 0);
			} finally {
				socket?.terminate();
				stopTestServer(server);
				await Promise.all([primary.close(), peer.close()]);
			}
		});

		test('viewers publish signed presence while session and sequence spoofing fail closed', async () => {
			const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
			await store.migrate();
			const id = crypto.randomUUID();
			const workspace = `workspace_${id}`;
			const object = `object_${id}`;
			const generation = `generation_${id}`;
			const owner = keyPair();
			const viewer = keyPair();
			const ownerToken = randomBytes(32).toString('base64url');
			const viewerToken = randomBytes(32).toString('base64url');
			const ownerActor = {
				deviceId: `owner_${id}`,
				accountId: `owner_account_${id}`,
				publicKey: owner.publicKey,
			};
			const viewerActor = {
				deviceId: `viewer_${id}`,
				accountId: `viewer_account_${id}`,
				publicKey: viewer.publicKey,
			};
			let ownerSocket: ProbeSocket | undefined;
			let viewerSocket: ProbeSocket | undefined;
			let server: ReturnType<typeof Bun.serve> | undefined;
			try {
				await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${ownerActor.deviceId},${ownerActor.accountId},${ownerActor.publicKey}),(${viewerActor.deviceId},${viewerActor.accountId},${viewerActor.publicKey})`;
				await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(ownerToken)},${ownerActor.deviceId},now()+interval '1 hour'),(${digest(viewerToken)},${viewerActor.deviceId},now()+interval '1 hour')`;
				await store.createWorkspace(ownerActor, workspace);
				await store.createObject(ownerActor, workspace, object);
				await store.db`UPDATE noura_objects SET generation=${generation},document_mode='text' WHERE workspace_id=${workspace} AND id=${object}`;
				await store.db`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${workspace},${viewerActor.accountId},'viewer')`;
				const app = createApp(store, {
					origin: 'http://127.0.0.1:1900',
					realtime: true,
				});
				server = Bun.serve({
					hostname: '127.0.0.1',
					port: 0,
					fetch: app.fetch,
					websocket,
				});
				const ownerConnection = await connect(
					server.port!,
					workspace,
					ownerToken,
				);
				ownerSocket = ownerConnection.socket;
				const viewerConnection = await connect(
					server.port!,
					workspace,
					viewerToken,
				);
				viewerSocket = viewerConnection.socket;
				await subscriptions(store, 4);
				const value: EncryptedPresence = {
					version: 1,
					workspaceId: workspace,
					objectId: object,
					generation,
					epoch: 1,
					deviceId: viewerActor.deviceId,
					sessionId: viewerConnection.hello.sessionId,
					sequence: 1,
					nonce: randomBytes(12).toString('base64'),
					ciphertext: randomBytes(32).toString('base64'),
					signature: '',
				};
				value.signature = sign(
					null,
					presenceSigningBytes(value),
					viewer.keys.privateKey,
				).toString('base64');
				const received = nextMessage(
					ownerSocket,
					(
						message,
					): message is { type: 'presence'; presence: EncryptedPresence } =>
						!!message &&
						typeof message === 'object' &&
						(message as { type?: unknown }).type === 'presence',
				);
				viewerSocket.send(
					JSON.stringify({ type: 'presence', presence: value }),
				);
				expect((await received).presence).toEqual(value);

				let releaseWorkspaceLock!: () => void;
				let reportWorkspaceLock!: () => void;
				const workspaceLockReady = new Promise<void>((resolve) => {
					reportWorkspaceLock = resolve;
				});
				const releaseWorkspace = new Promise<void>((resolve) => {
					releaseWorkspaceLock = resolve;
				});
				const workspaceLock = store.transaction(async (tx) => {
					await tx`SELECT 1 FROM noura_workspaces WHERE id=${workspace} FOR UPDATE`;
					reportWorkspaceLock();
					await releaseWorkspace;
				});
				await workspaceLockReady;
				value.sequence = 2;
				value.signature = sign(
					null,
					presenceSigningBytes(value),
					viewer.keys.privateKey,
				).toString('base64');
				const delayedPresence = nextMessage(
					ownerSocket,
					(
						message,
					): message is { type: 'presence'; presence: EncryptedPresence } =>
						!!message &&
						typeof message === 'object' &&
						(message as { type?: unknown }).type === 'presence' &&
						(message as { presence?: { sequence?: unknown } }).presence
							?.sequence === 2,
				);
				const departed = nextMessage(
					ownerSocket,
					(
						message,
					): message is {
						type: 'presence-left';
						deviceId: string;
						sessionId: string;
					} =>
						!!message &&
						typeof message === 'object' &&
						(message as { type?: unknown }).type === 'presence-left' &&
						(message as { sessionId?: unknown }).sessionId ===
							viewerConnection.hello.sessionId,
				);
				viewerSocket.send(
					JSON.stringify({ type: 'presence', presence: value }),
				);
				await sleep(25);
				const cleanClose = nextClose(viewerSocket);
				viewerSocket.close(1000);
				releaseWorkspaceLock();
				await workspaceLock;
				expect((await cleanClose).code).toBe(1000);
				expect((await delayedPresence).presence.sequence).toBe(2);
				expect((await departed).deviceId).toBe(viewerActor.deviceId);

				const replacement = await connect(server.port!, workspace, viewerToken);
				viewerSocket = replacement.socket;
				await subscriptions(store, 4);
				const errorMessage = nextMessage(
					viewerSocket,
					(message): message is { type: 'error'; code: string } =>
						!!message &&
						typeof message === 'object' &&
						(message as { type?: unknown }).type === 'error',
				);
				const closed = nextClose(viewerSocket);
				viewerSocket.send(
					JSON.stringify({ type: 'presence', presence: value }),
				);
				expect((await errorMessage).code).toBe('sync.invalid_presence');
				expect((await closed).code).toBe(4403);
			} finally {
				ownerSocket?.terminate();
				viewerSocket?.terminate();
				stopTestServer(server);
				await store.close();
			}
		});
	},
);
