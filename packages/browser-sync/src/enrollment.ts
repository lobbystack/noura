/**
 * Browser device challenge and enrollment against the Noura sync service.
 *
 * Enrollment proves possession of the device Ed25519 signing key over the
 * `noura.device.enroll.web` version 1 tuple and binds the browser `x25519:`
 * recipient. Registration grants no workspace content key; a trusted device
 * must still approve the device and deliver keys.
 *
 * The server is never a trust source. Callers verify delivered envelopes against
 * locally pinned signer keys in {@link receiveKeys}.
 */

import { enrollmentProof } from '@noura/sync-key-envelope';
import { encodeBase64 } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { ensureResponseOk, readJson, type FetchLike } from './http';
import type { DeviceIdentity } from './identity';

/** Parsed response from `POST /v1/device-challenges`. */
export interface RequestDeviceChallengeResult {
	/** Server-issued, single-use, short-lived challenge. */
	challenge: string;
	/** Account the challenge was issued to. */
	accountId: string;
	/** Remaining validity in seconds. */
	expiresIn: number;
}

function asRecord(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response had an unexpected shape',
		);
	}
	return value as Record<string, unknown>;
}

function requireString(value: unknown, field: string): string {
	if (typeof value !== 'string' || value.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`response was missing ${field}`,
		);
	}
	return value;
}

/**
 * Request a single-use device enrollment challenge.
 *
 * The supplied `fetch` must be bound to the account origin (or be the global
 * `fetch` in a same-origin browser context); the request is sent to
 * `/v1/device-challenges` with credentials included.
 */
export async function requestDeviceChallenge(
	fetchImpl: FetchLike,
): Promise<RequestDeviceChallengeResult> {
	const response = await fetchImpl('/v1/device-challenges', {
		method: 'POST',
		credentials: 'include',
		headers: { accept: 'application/json' },
	});
	ensureResponseOk(response);
	const data = asRecord(await readJson(response));
	const challenge = requireString(data.challenge, 'challenge');
	const accountId = requireString(data.accountId, 'accountId');
	if (
		typeof data.expiresIn !== 'number' ||
		!Number.isInteger(data.expiresIn) ||
		data.expiresIn < 0
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was missing expiresIn',
		);
	}
	return { challenge, accountId, expiresIn: data.expiresIn };
}

/** Inputs for {@link enrollBrowserDevice}. */
export interface EnrollBrowserDeviceInput {
	/** Account origin the proof is bound to. */
	origin: string;
	/** Account the device enrolls under. */
	accountId: string;
	/** Server-issued single-use challenge. */
	challenge: string;
	/** Unlocked device identity whose signing key produces the proof. */
	identity: DeviceIdentity;
	/** Injected transport. */
	fetch: FetchLike;
}

/**
 * Enroll the browser device and return its bearer token.
 *
 * Signs `[noura.device.enroll.web,1,origin,accountId,deviceId,base64(publicKey),
 * recipient,challenge]` with the device signing key and posts
 * `{challenge, deviceId, publicKey, proof, encryptionRecipient}` to
 * `/v1/devices`. `publicKey` is the raw 32-byte Ed25519 public key as base64, as
 * the server expects.
 *
 * The returned token is secret. Persist it only by re-sealing the identity with
 * {@link sealIdentity}; never write it outside a wrapped bundle.
 */
export async function enrollBrowserDevice(
	input: EnrollBrowserDeviceInput,
): Promise<string> {
	const publicKey = encodeBase64(input.identity.signingPublic);
	const proof = await enrollmentProof({
		origin: input.origin,
		account_id: input.accountId,
		device_id: input.identity.deviceId,
		signing_secret: encodeBase64(input.identity.signingSeed),
		signing_public: publicKey,
		recipient: input.identity.recipient,
		challenge: input.challenge,
	});
	const body = {
		challenge: input.challenge,
		deviceId: input.identity.deviceId,
		publicKey,
		proof: encodeBase64(proof),
		encryptionRecipient: input.identity.recipient,
	};
	const response = await input.fetch(
		new URL('/v1/devices', input.origin).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: {
				'content-type': 'application/json',
				accept: 'application/json',
			},
			body: JSON.stringify(body),
		},
	);
	ensureResponseOk(response);
	const data = asRecord(await readJson(response));
	const token = requireString(data.token, 'token');
	if (
		data.deviceId !== undefined &&
		data.deviceId !== input.identity.deviceId
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response device identifier did not match',
		);
	}
	return token;
}
