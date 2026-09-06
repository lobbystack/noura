import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { sign } from 'node:crypto';
import { getMigrations } from 'better-auth/db/migration';
import { createAuth } from './auth';
import { createApp } from './app';
import { SyncStore } from './store';
import { fixture } from './protocol.test';

const url = process.env.NOURA_TEST_DATABASE_URL;
describe.skipIf(!url)('account to device enrollment', () => {
	const suffix = crypto.randomUUID();
	const email = `test-${suffix}@example.invalid`;
	const origin = 'http://localhost:1900';
	let delivered = '';
	const identity = createAuth(
		{
			databaseUrl: url!,
			origin,
			port: 1900,
			host: '127.0.0.1',
			authSecret: 'test-only-secret-that-is-at-least-32-characters',
			smtpUrl: 'smtp://localhost:1025',
			mailFrom: 'noura@example.invalid',
			allowedEmails: new Set([email]),
		},
		async (_email, link) => {
			delivered = link;
		},
	);
	const store = new SyncStore(url!);
	const app = createApp(store, { origin, auth: identity.auth });
	beforeAll(async () => {
		await store.migrate();
		await store.db.begin(async (tx) => {
			await tx`SELECT pg_advisory_xact_lock(192837466)`;
			await (await getMigrations(identity.auth.options)).runMigrations();
		});
	});
	afterAll(async () => {
		await identity.close();
		await store.close();
	});
	test('email login, fresh key proof, session renewal, replay rejection and revocation', async () => {
		const login = await app.request('/api/auth/sign-in/magic-link', {
			method: 'POST',
			headers: { Origin: origin, 'Content-Type': 'application/json' },
			body: JSON.stringify({ email, callbackURL: `${origin}/account` }),
		});
		expect(login.status).toBe(200);
		expect(delivered).toContain('/api/auth/magic-link/verify');
		const verification = await app.request(delivered);
		expect(verification.status).toBe(302);
		const cookie = verification.headers
			.getSetCookie()
			.map((x) => x.split(';')[0])
			.join('; ');
		const headers = {
			Cookie: cookie,
			Origin: origin,
			'Content-Type': 'application/json',
		};
		const deviceId = `device_${suffix}`;
		const f = fixture(deviceId);
		async function enroll() {
			const challengeResponse = await app.request('/v1/device-challenges', {
				method: 'POST',
				headers,
			});
			expect(challengeResponse.status).toBe(200);
			const { challenge, accountId } = (await challengeResponse.json()) as {
				challenge: string;
				accountId: string;
			};
			const proof = sign(
				null,
				Buffer.from(
					JSON.stringify([
						'noura.device.enroll',
						origin,
						accountId,
						deviceId,
						f.publicKey,
						challenge,
					]),
				),
				f.keys.privateKey,
			).toString('base64');
			const body = JSON.stringify({
				deviceId,
				publicKey: f.publicKey,
				challenge,
				proof,
			});
			const response = await app.request('/v1/devices', {
				method: 'POST',
				headers,
				body,
			});
			return { response, body };
		}
		const first = await enroll();
		expect(first.response.status).toBe(201);
		const { token } = (await first.response.json()) as { token: string };
		expect((await store.authenticate(token)).deviceId).toBe(deviceId);
		const replay = await app.request('/v1/devices', {
			method: 'POST',
			headers,
			body: first.body,
		});
		expect(replay.status).toBe(403);
		const second = await enroll();
		expect(second.response.status).toBe(201);
		const next = (await second.response.json()) as { token: string };
		await expect(store.authenticate(token)).rejects.toThrow(
			'sync.unauthorized',
		);
		const revoked = await app.request(`/v1/devices/${deviceId}`, {
			method: 'DELETE',
			headers: { Authorization: `Bearer ${next.token}` },
		});
		expect(revoked.status).toBe(204);
		await expect(store.authenticate(next.token)).rejects.toThrow(
			'sync.unauthorized',
		);
		const denied = await enroll();
		expect(denied.response.status).toBe(403);
	});
	test('uninvited emails do not receive mail', async () => {
		delivered = '';
		const response = await app.request('/api/auth/sign-in/magic-link', {
			method: 'POST',
			headers: { Origin: origin, 'Content-Type': 'application/json' },
			body: JSON.stringify({ email: `outsider-${suffix}@example.invalid` }),
		});
		expect(response.status).toBe(200);
		expect(delivered).toBe('');
	});
});
