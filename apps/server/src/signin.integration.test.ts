import { describe, expect, test } from 'bun:test';
import { getMigrations } from 'better-auth/db/migration';
import { createAuth } from './auth';
import { createApp } from './app';
import { SyncStore } from './store';

describe.skipIf(
	!process.env.NOURA_TEST_DATABASE_URL ||
		!process.env.NOURA_NATIVE_SIGNIN_PROBE,
)('native browser-approved device sign-in', () => {
	test('native client enrolls its bound recipient and stores tokens outside its public output', async () => {
		const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
		await store.migrate();
		let app: ReturnType<typeof createApp> | undefined;
		const server = Bun.serve({
			hostname: '127.0.0.1',
			port: 0,
			fetch: (request) =>
				app?.fetch(request) ?? new Response(null, { status: 503 }),
		});
		const origin = `http://127.0.0.1:${server.port}`;
		const email = `native-signin-${crypto.randomUUID()}@example.invalid`;
		let delivered = '';
		const identity = createAuth(
			{
				databaseUrl: process.env.NOURA_TEST_DATABASE_URL!,
				origin,
				port: server.port!,
				host: '127.0.0.1',
				authSecret: 'test-only-native-signin-secret-at-least-32-characters',
				smtpUrl: 'smtp://localhost:1025',
				mailFrom: 'test@example.invalid',
				allowedEmails: new Set([email]),
			},
			async (_email, link) => {
				delivered = link;
			},
		);
		let child: ReturnType<typeof Bun.spawn> | undefined;
		try {
			await store.db.begin(async (tx) => {
				await tx`SELECT pg_advisory_xact_lock(192837466)`;
				await (await getMigrations(identity.auth.options)).runMigrations();
			});
			app = createApp(store, { origin, auth: identity.auth });
			await app.request('/api/auth/sign-in/magic-link', {
				method: 'POST',
				headers: { Origin: origin, 'Content-Type': 'application/json' },
				body: JSON.stringify({ email, callbackURL: `${origin}/account` }),
			});
			const verified = await app.request(delivered);
			const cookie = verified.headers
				.getSetCookie()
				.map((value) => value.split(';')[0])
				.join('; ');
			child = Bun.spawn([process.env.NOURA_NATIVE_SIGNIN_PROBE!], {
				stdin: new Blob([origin]),
				stdout: 'pipe',
				stderr: 'pipe',
			});
			const stream = child.stdout as ReadableStream<Uint8Array>;
			const reader = stream.getReader();
			let output = '';
			const decoder = new TextDecoder();
			while (!output.includes('\n')) {
				const { value, done } = await reader.read();
				if (done)
					throw new Error('Native sign-in ended before presenting a code');
				output += decoder.decode(value, { stream: true });
			}
			const info = JSON.parse(output.split('\n')[0]!);
			expect(info.verificationUri).toContain(
				`${origin}/account/device?user_code=`,
			);
			expect(output).not.toContain('device_code');
			const headers = {
				Cookie: cookie,
				Origin: origin,
				'Content-Type': 'application/json',
			};
			const review = await app.request(
				`/api/auth/device?user_code=${encodeURIComponent(info.userCode)}`,
				{ headers },
			);
			expect(review.status).toBe(200);
			const approval = await app.request('/api/auth/device/approve', {
				method: 'POST',
				headers,
				body: JSON.stringify({ userCode: info.userCode }),
			});
			expect(approval.status).toBe(200);
			for (;;) {
				const { value, done } = await reader.read();
				if (done) break;
				output += decoder.decode(value, { stream: true });
			}
			const stderr = await new Response(child.stderr as ReadableStream).text();
			expect(stderr).toBe('');
			expect(await child.exited).toBe(0);
			const result = JSON.parse(output.trim().split('\n').at(-1)!);
			expect(result.connected).toBe(true);
			expect(result.coordinatorRestart).toBe(true);
			const envelopes =
				await store.db`SELECT wrapped_key FROM noura_key_envelopes WHERE workspace_id=${result.workspaceId}`;
			expect(envelopes.length).toBe(1);
			const operations =
				await store.db`SELECT object_id FROM noura_operations WHERE workspace_id=${result.workspaceId}`;
			expect(operations.length).toBe(3);
			expect(new Set(operations.map((row) => row.object_id)).size).toBe(1);
			const [device] =
				await store.db`SELECT encryption_recipient FROM noura_devices WHERE id=${result.deviceId}`;
			expect(device!.encryption_recipient).toMatch(/^age1/);
			expect(output).not.toContain('token');
		} finally {
			child?.kill();
			await server.stop(true);
			await identity.close();
			await store.close();
		}
	}, 60_000);
});
