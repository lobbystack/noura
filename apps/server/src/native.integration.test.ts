import { describe, expect, test } from 'bun:test';
import { createPrivateKey, createPublicKey, randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { SyncStore } from './store';
import { createApp } from './app';
import { digest } from './protocol';

describe.skipIf(
	!process.env.NOURA_TEST_DATABASE_URL || !process.env.NOURA_NATIVE_PROBE,
)('native Rust clients against the HTTP server', () => {
	test('three file workspaces exchange ciphertext and preserve an offline conflict', async () => {
		const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
		await store.migrate();
		const device = `native_${crypto.randomUUID()}`;
		const privateKey = createPrivateKey({
			key: Buffer.concat([
				Buffer.from('302e020100300506032b657004220420', 'hex'),
				Buffer.alloc(32, 7),
			]),
			format: 'der',
			type: 'pkcs8',
		});
		const publicKey = createPublicKey(privateKey)
			.export({ format: 'der', type: 'spki' })
			.subarray(-32)
			.toString('base64');
		const token = randomBytes(32).toString('base64url');
		await store.db`INSERT INTO noura_devices(id,account_id,public_key) VALUES(${device},${device},${publicKey})`;
		await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(token)},${device},now()+interval '1 hour')`;
		const app = createApp(store, { origin: 'http://localhost:1900' });
		const server = Bun.serve({
			hostname: '127.0.0.1',
			port: 0,
			fetch: app.fetch,
		});
		try {
			const child = Bun.spawn([process.env.NOURA_NATIVE_PROBE!], {
				cwd: fileURLToPath(new URL('../../..', import.meta.url)),
				stdin: new Blob([
					JSON.stringify({
						origin: `http://127.0.0.1:${server.port}`,
						token,
						device,
					}),
				]),
				stdout: 'pipe',
				stderr: 'pipe',
			});
			const [code, stdout, stderr] = await Promise.all([
				child.exited,
				new Response(child.stdout).text(),
				new Response(child.stderr).text(),
			]);
			expect(stderr).toBe('');
			expect(code).toBe(0);
			const result = JSON.parse(stdout);
			expect(result.clients).toBe(3);
			expect(result.cursor).toBe('2');
			const rows =
				await store.db`SELECT * FROM noura_operations WHERE workspace_id=${result.workspace}`;
			expect(rows).toHaveLength(2);
			expect(JSON.stringify(rows)).not.toContain('native-plaintext-sentinel');
			expect(JSON.stringify(rows)).not.toContain('private-note.md');
		} finally {
			await server.stop(true);
			await store.close();
		}
	}, 60_000);
});
