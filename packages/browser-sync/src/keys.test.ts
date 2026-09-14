import { describe, expect, test } from 'bun:test';
import { wrapKey, type WebKeyEnvelope } from '@noura/sync-key-envelope';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	createDeviceIdentity,
	decodeBase64,
	encodeBase64,
	receiveKeys,
	unlockDeviceIdentity,
	type DeviceIdentity,
	type FetchLike,
} from './index';

const PASSPHRASE = 'correct horse battery staple';
const ORIGIN = 'https://app.noura.example';
const WORKSPACE = 'workspace';

// Fixed signer material from the browser-key-v1 fixture.
const SIGNING_SECRET = 'AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=';
const SIGNING_PUBLIC = 'iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=';
const OBJECT_KEY = 'AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=';
const EPHEMERAL_SECRET = 'BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ=';
const SALT = 'BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU=';
const NONCE = 'BgYGBgYGBgYGBgYG';

async function makeIdentity(): Promise<DeviceIdentity> {
	const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
	return unlockDeviceIdentity(bundle, PASSPHRASE);
}

async function makeEnvelope(
	identity: DeviceIdentity,
	objectId: string,
	epoch = 1,
): Promise<WebKeyEnvelope> {
	return wrapKey({
		workspace_id: WORKSPACE,
		object_id: objectId,
		epoch,
		signing_device: 'device_signer',
		device_id: identity.deviceId,
		signing_secret: SIGNING_SECRET,
		recipient_public: encodeBase64(identity.x25519Public),
		object_key: OBJECT_KEY,
		ephemeral_secret: EPHEMERAL_SECRET,
		salt: SALT,
		nonce: NONCE,
	});
}

function row(envelope: WebKeyEnvelope): Record<string, unknown> {
	return {
		objectId: envelope.object_id,
		epoch: String(envelope.epoch),
		deviceId: envelope.device_id,
		wrappedKey: envelope.wrapped_key,
		signingDevice: envelope.signing_device,
		signature: envelope.signature,
		construction: 'web',
		recipientPublicKey: envelope.recipient_public_key,
		ephemeralPublicKey: envelope.ephemeral_public_key,
		salt: envelope.salt,
		nonce: envelope.nonce,
		signingPublicKey: SIGNING_PUBLIC,
	};
}

function pinnedSigners(): ReadonlyMap<string, Uint8Array> {
	return new Map([['device_signer', decodeBase64(SIGNING_PUBLIC)]]);
}

function jsonFetch(payload: unknown): FetchLike {
	return async () => Response.json(payload);
}

describe('receiveKeys', () => {
	test('verifies a pinned web envelope and unwraps its object key', async () => {
		const identity = await makeIdentity();
		const envelope = await makeEnvelope(identity, 'object-1');

		const result = await receiveKeys({
			origin: ORIGIN,
			token: 'device-token',
			workspaceId: WORKSPACE,
			deviceId: identity.deviceId,
			identity,
			pinnedSigners: pinnedSigners(),
			fetch: jsonFetch({ envelopes: [row(envelope)], hasMore: false }),
		});

		expect(result.unsupported).toEqual([]);
		const key = result.keys.get('object-1');
		expect(key).toBeDefined();
		expect(Array.from(key!)).toEqual(Array.from(decodeBase64(OBJECT_KEY)));
	});

	test('rejects an envelope from an unpinned signer', async () => {
		const identity = await makeIdentity();
		const envelope = await makeEnvelope(identity, 'object-1');

		let error: unknown;
		try {
			await receiveKeys({
				origin: ORIGIN,
				token: 'device-token',
				workspaceId: WORKSPACE,
				deviceId: identity.deviceId,
				identity,
				pinnedSigners: new Map(),
				fetch: jsonFetch({ envelopes: [row(envelope)], hasMore: false }),
			});
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.UnpinnedSigner,
		);
	});

	test('rejects an envelope addressed to a different recipient', async () => {
		const identity = await makeIdentity();
		const envelope = await makeEnvelope(identity, 'object-1');
		const tampered: WebKeyEnvelope = {
			...envelope,
			recipient_public_key: encodeBase64(new Uint8Array(32).fill(7)),
		};

		let error: unknown;
		try {
			await receiveKeys({
				origin: ORIGIN,
				token: 'device-token',
				workspaceId: WORKSPACE,
				deviceId: identity.deviceId,
				identity,
				pinnedSigners: pinnedSigners(),
				fetch: jsonFetch({ envelopes: [row(tampered)], hasMore: false }),
			});
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.RecipientMismatch,
		);
	});

	test('rejects an envelope with a tampered signature', async () => {
		const identity = await makeIdentity();
		const envelope = await makeEnvelope(identity, 'object-1');
		const tampered: WebKeyEnvelope = {
			...envelope,
			signature: encodeBase64(new Uint8Array(64)),
		};
		await expect(
			receiveKeys({
				origin: ORIGIN,
				token: 'device-token',
				workspaceId: WORKSPACE,
				deviceId: identity.deviceId,
				identity,
				pinnedSigners: pinnedSigners(),
				fetch: jsonFetch({ envelopes: [row(tampered)], hasMore: false }),
			}),
		).rejects.toMatchObject({ code: 'browser_sync_invalid_envelope' });
	});

	test('paginates with afterObject and afterEpoch', async () => {
		const identity = await makeIdentity();
		const first = await makeEnvelope(identity, 'object-1');
		const second = await makeEnvelope(identity, 'object-2');
		const seen: string[] = [];

		const fetchImpl: FetchLike = async (input) => {
			const url = new URL(String(input));
			const afterObject = url.searchParams.get('afterObject') ?? '';
			seen.push(afterObject);
			if (afterObject === '') {
				return Response.json({ envelopes: [row(first)], hasMore: true });
			}
			return Response.json({ envelopes: [row(second)], hasMore: false });
		};

		const result = await receiveKeys({
			origin: ORIGIN,
			token: 'device-token',
			workspaceId: WORKSPACE,
			deviceId: identity.deviceId,
			identity,
			pinnedSigners: pinnedSigners(),
			fetch: fetchImpl,
		});

		expect(seen).toEqual(['', 'object-1']);
		expect(Array.from(result.keys.keys()).sort()).toEqual([
			'object-1',
			'object-2',
		]);
	});

	test('returns an unsupported marker for age envelopes', async () => {
		const identity = await makeIdentity();
		const ageRow = {
			objectId: 'object-age',
			epoch: '1',
			deviceId: identity.deviceId,
			wrappedKey: 'unused',
			signingDevice: 'device_signer',
			signature: 'unused',
			construction: 'age',
		};

		const result = await receiveKeys({
			origin: ORIGIN,
			token: 'device-token',
			workspaceId: WORKSPACE,
			deviceId: identity.deviceId,
			identity,
			pinnedSigners: pinnedSigners(),
			fetch: jsonFetch({ envelopes: [ageRow], hasMore: false }),
		});

		expect(result.keys.size).toBe(0);
		expect(result.unsupported).toEqual([
			{
				objectId: 'object-age',
				deviceId: identity.deviceId,
				epoch: 1,
				construction: 'age',
				code: 'unsupported_envelope',
			},
		]);
	});

	test('sends the bearer token, recipient device, and cursors', async () => {
		const identity = await makeIdentity();
		const envelope = await makeEnvelope(identity, 'object-1');
		let capturedUrl = '';
		let capturedAuth: string | undefined;
		const fetchImpl: FetchLike = async (input, init) => {
			capturedUrl = String(input);
			capturedAuth =
				new Headers(init?.headers).get('authorization') ?? undefined;
			return Response.json({ envelopes: [row(envelope)], hasMore: false });
		};

		await receiveKeys({
			origin: ORIGIN,
			token: 'secret-token',
			workspaceId: WORKSPACE,
			deviceId: identity.deviceId,
			identity,
			pinnedSigners: pinnedSigners(),
			fetch: fetchImpl,
		});

		const url = new URL(capturedUrl);
		expect(url.pathname).toBe(`/v1/workspaces/${WORKSPACE}/keys`);
		expect(url.searchParams.get('device')).toBe(identity.deviceId);
		expect(url.searchParams.get('afterObject')).toBe('');
		expect(url.searchParams.get('afterEpoch')).toBe('0');
		expect(capturedAuth).toBe('Bearer secret-token');
	});
});
