import { describe, expect, test } from 'bun:test';
import {
	createDeviceIdentity,
	decodeBase64,
	encodeBase64,
	enrollBrowserDevice,
	requestDeviceChallenge,
	unlockDeviceIdentity,
	type FetchLike,
} from './index';

const PASSPHRASE = 'correct horse battery staple';
const ORIGIN = 'https://app.noura.example';

interface Call {
	input: RequestInfo | URL;
	init: RequestInit | undefined;
}

function recordingFetch(respond: (call: Call) => Response): {
	fetch: FetchLike;
	calls: Call[];
} {
	const calls: Call[] = [];
	const fetchImpl: FetchLike = async (input, init) => {
		const call: Call = { input, init };
		calls.push(call);
		return respond(call);
	};
	return { fetch: fetchImpl, calls };
}

describe('device challenge', () => {
	test('posts to /v1/device-challenges with credentials and parses the result', async () => {
		const { fetch, calls } = recordingFetch(() =>
			Response.json({
				challenge: 'c1',
				accountId: 'account_owner',
				expiresIn: 300,
			}),
		);

		const result = await requestDeviceChallenge(fetch);
		expect(result).toEqual({
			challenge: 'c1',
			accountId: 'account_owner',
			expiresIn: 300,
		});
		expect(String(calls[0]!.input)).toBe('/v1/device-challenges');
		expect(calls[0]!.init?.method).toBe('POST');
		expect(calls[0]!.init?.credentials).toBe('include');
	});

	test('surfaces an unauthorized response as a structured error', async () => {
		const { fetch } = recordingFetch(() => new Response(null, { status: 401 }));
		await expect(requestDeviceChallenge(fetch)).rejects.toMatchObject({
			code: 'browser_sync_unauthorized',
		});
	});
});

describe('browser device enrollment', () => {
	test('signs the exact enrollment tuple and returns the device token', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const { fetch, calls } = recordingFetch(() =>
			Response.json(
				{
					deviceId: identity.deviceId,
					token: 'device-token',
					expiresIn: 604800,
				},
				{ status: 201 },
			),
		);

		const token = await enrollBrowserDevice({
			origin: ORIGIN,
			accountId: 'account_owner',
			challenge: 'challenge-one',
			identity,
			fetch,
		});
		expect(token).toBe('device-token');

		const call = calls[0]!;
		expect(String(call.input)).toBe(`${ORIGIN}/v1/devices`);
		expect(call.init?.method).toBe('POST');
		expect(call.init?.credentials).toBe('include');

		const body = JSON.parse(String(call.init?.body)) as Record<string, string>;
		expect(body).toEqual({
			challenge: 'challenge-one',
			deviceId: identity.deviceId,
			publicKey: encodeBase64(identity.signingPublic),
			proof: expect.any(String),
			encryptionRecipient: identity.recipient,
		});

		const message = new TextEncoder().encode(
			JSON.stringify([
				'noura.device.enroll.web',
				1,
				ORIGIN,
				'account_owner',
				identity.deviceId,
				body.publicKey,
				identity.recipient,
				'challenge-one',
			]),
		);
		const verifyingKey = await crypto.subtle.importKey(
			'raw',
			identity.signingPublic,
			{ name: 'Ed25519' },
			false,
			['verify'],
		);
		const valid = await crypto.subtle.verify(
			{ name: 'Ed25519' },
			verifyingKey,
			decodeBase64(body.proof!),
			message,
		);
		expect(valid).toBe(true);
	});

	test('rejects a response whose device identifier does not match', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const { fetch } = recordingFetch(() =>
			Response.json({ deviceId: 'someone-else', token: 't' }),
		);
		await expect(
			enrollBrowserDevice({
				origin: ORIGIN,
				accountId: 'account_owner',
				challenge: 'challenge-one',
				identity,
				fetch,
			}),
		).rejects.toMatchObject({ code: 'browser_sync_invalid_response' });
	});
});
