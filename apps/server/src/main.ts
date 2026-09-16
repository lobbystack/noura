import { config } from './config';
import { createAuth } from './auth';
import { createApp } from './app';
import { SyncStore } from './store';
import { BlobService } from './blobs';
import { retryReady } from './startup';
import { fileURLToPath } from 'node:url';
import { websocket } from 'hono/bun';

const settings = config();
const store = new SyncStore(settings.databaseUrl);
const identity = createAuth(settings);
// Migrations are an explicit deployment step, never raced at application startup.
// The database may still be waking when this process starts, so wait for the
// schema probe with bounded backoff instead of crashing into a restart loop.
await retryReady(() => store.ready(), {
	onRetry: (error, attempt, delayMs) =>
		console.warn(
			`Database not ready (attempt ${attempt}): ${
				error instanceof Error ? error.message : 'unknown error'
			}; retrying in ${delayMs}ms`,
		),
});
let s3: Bun.S3Client | undefined;
if (process.env.S3_BUCKET) {
	const accessKeyId = process.env.S3_ACCESS_KEY_ID;
	const secretAccessKey = process.env.S3_SECRET_ACCESS_KEY;
	const endpoint = process.env.S3_ENDPOINT;
	if (!accessKeyId || !secretAccessKey)
		throw new Error('S3 credentials are required with S3_BUCKET');
	if (endpoint) {
		const url = new URL(endpoint);
		if (
			url.username ||
			url.password ||
			!(
				url.protocol === 'https:' ||
				(url.protocol === 'http:' &&
					['localhost', '127.0.0.1'].includes(url.hostname))
			)
		)
			throw new Error(
				'S3_ENDPOINT must use HTTPS (HTTP is allowed only on loopback)',
			);
	}
	try {
		s3 = new Bun.S3Client({
			bucket: process.env.S3_BUCKET,
			region: process.env.S3_REGION ?? 'us-east-1',
			accessKeyId,
			secretAccessKey,
			...(endpoint ? { endpoint } : {}),
		});
	} catch {
		throw new Error('Invalid S3 configuration');
	}
}
const blobs = await BlobService.open(
	store,
	process.env.BLOB_ROOT ?? './data/blobs',
	s3,
);
const app = createApp(store, {
	origin: settings.origin,
	auth: identity.auth,
	blobs,
	webRoot:
		process.env.WEB_ROOT ?? fileURLToPath(new URL('./public', import.meta.url)),
});
const server = Bun.serve({
	hostname: settings.host,
	port: settings.port,
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
console.info(`Noura sync listening on port ${server.port}`);
let stopping = false;
async function stop() {
	if (stopping) return;
	stopping = true;
	await server.stop();
	await identity.close();
	await store.close();
}
process.on('SIGTERM', () => void stop());
process.on('SIGINT', () => void stop());
