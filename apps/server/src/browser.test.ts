import { describe, expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import deviceFixture from '../../../docs/workspace-format/fixtures/browser-device-v1.json';
import {
	browserEnrollmentSigningBytes,
	browserKeySigningBytes,
	browserRecipient,
	browserRecipientMatches,
	decodeBrowserRecipient,
	parseKeyEnvelope,
	verifyEd25519,
	verifyKeyEnvelopeSignature,
	type BrowserKeyTupleFields,
} from './browser';

function signer() {
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	return { keys, publicKey };
}

function webEnvelope(
	privateKey: Parameters<typeof sign>[2],
	overrides: Partial<BrowserKeyTupleFields> = {},
) {
	const fields: BrowserKeyTupleFields = {
		workspaceId: 'workspace',
		objectId: 'object',
		epoch: 1,
		signingDevice: 'device_signer',
		deviceId: 'device_browser',
		recipientPublicKey: randomBytes(32).toString('base64'),
		ephemeralPublicKey: randomBytes(32).toString('base64'),
		salt: randomBytes(32).toString('base64'),
		nonce: randomBytes(12).toString('base64'),
		wrappedKey: randomBytes(80).toString('base64'),
		...overrides,
	};
	return {
		...fields,
		construction: 'web' as const,
		signature: sign(null, browserKeySigningBytes(fields), privateKey).toString(
			'base64',
		),
	};
}

function ageEnvelope(
	privateKey: Parameters<typeof sign>[2],
	overrides: Record<string, unknown> = {},
) {
	const fields = {
		workspaceId: 'workspace',
		objectId: 'object',
		epoch: 1,
		signingDevice: 'device_signer',
		deviceId: 'device_age',
		wrappedKey: randomBytes(80).toString('base64'),
		...overrides,
	};
	return {
		...fields,
		signature: sign(
			null,
			Buffer.from(
				JSON.stringify([
					'noura.sync.key',
					1,
					fields.workspaceId,
					fields.objectId,
					fields.epoch,
					fields.signingDevice,
					fields.deviceId,
					fields.wrappedKey,
				]),
			),
			privateKey,
		).toString('base64'),
	};
}

describe('browser device recipient encoding', () => {
	test('accepts the shared fixture recipients and rejects every other form', () => {
		for (const vector of deviceFixture.recipients.valid)
			expect(decodeBrowserRecipient(vector.recipient)).toHaveLength(32);
		for (const vector of deviceFixture.recipients.invalid)
			expect(() => decodeBrowserRecipient(vector.recipient)).toThrow(
				'sync.invalid_recipient',
			);
		for (const value of [null, 42, {}, 'x25519:', 'x25519:!!!!'])
			expect(browserRecipient(value)).toBeUndefined();
	});

	test('matches a stored recipient only when the X25519 bytes agree', () => {
		const bytes = randomBytes(32).toString('base64');
		const recipient = `x25519:${bytes}`;
		expect(browserRecipientMatches(recipient, bytes)).toBe(true);
		expect(
			browserRecipientMatches(recipient, randomBytes(32).toString('base64')),
		).toBe(false);
		expect(browserRecipientMatches(`age1${'q'.repeat(58)}`, bytes)).toBe(false);
		expect(browserRecipientMatches(null, bytes)).toBe(false);
	});
});

describe('browser enrollment proof', () => {
	test('verifies only the exact noura.device.enroll.web tuple', () => {
		const { keys, publicKey } = signer();
		const input = {
			origin: 'https://app.noura.example',
			accountId: 'account_owner',
			deviceId: 'device_browser',
			publicKey: randomBytes(32).toString('base64'),
			recipient: `x25519:${randomBytes(32).toString('base64')}`,
			challenge: 'challenge_browser_device_1',
		};
		const proof = sign(
			null,
			browserEnrollmentSigningBytes(input),
			keys.privateKey,
		).toString('base64');
		expect(
			verifyEd25519(publicKey, browserEnrollmentSigningBytes(input), proof),
		).toBe(true);
		expect(
			verifyEd25519(
				publicKey,
				browserEnrollmentSigningBytes({ ...input, challenge: 'other' }),
				proof,
			),
		).toBe(false);
	});
});

describe('browser key envelope validation', () => {
	test('parses and verifies a browser envelope; the tuple binds every field', () => {
		const { keys, publicKey } = signer();
		const value = webEnvelope(keys.privateKey);
		const parsed = parseKeyEnvelope(value);
		expect(parsed).toMatchObject({
			construction: 'web',
			workspaceId: 'workspace',
			objectId: 'object',
			deviceId: 'device_browser',
		});
		expect(verifyKeyEnvelopeSignature(parsed, publicKey)).toBe(true);
		expect(verifyKeyEnvelopeSignature(parsed, signer().publicKey)).toBe(false);
		for (const changed of [
			{ recipientPublicKey: randomBytes(32).toString('base64') },
			{ ephemeralPublicKey: randomBytes(32).toString('base64') },
			{ salt: randomBytes(32).toString('base64') },
			{ nonce: randomBytes(12).toString('base64') },
			{ wrappedKey: randomBytes(80).toString('base64') },
			{ deviceId: 'other_device' },
			{ epoch: 2 },
		])
			expect(
				verifyKeyEnvelopeSignature(
					parseKeyEnvelope({ ...value, ...changed }),
					publicKey,
				),
			).toBe(false);
	});

	test('preserves native age parsing and signature behavior', () => {
		const { keys, publicKey } = signer();
		const value = ageEnvelope(keys.privateKey);
		const parsed = parseKeyEnvelope(value);
		expect(parsed.construction).toBe('age');
		expect(parsed.recipientPublicKey).toBeNull();
		expect(verifyKeyEnvelopeSignature(parsed, publicKey)).toBe(true);
		expect(
			verifyKeyEnvelopeSignature(
				parseKeyEnvelope({ ...value, epoch: 2 }),
				publicKey,
			),
		).toBe(false);
		// An explicit age discriminator is accepted and means the same construction.
		expect(
			parseKeyEnvelope({ ...value, construction: 'age' }).construction,
		).toBe('age');
	});

	test('rejects unknown constructions and mixed, missing or extra fields', () => {
		const { keys } = signer();
		const value = webEnvelope(keys.privateKey);
		expect(() => parseKeyEnvelope({ ...value, construction: 'wasm' })).toThrow(
			'sync.invalid_key',
		);
		expect(() =>
			parseKeyEnvelope({ ...value, construction: undefined }),
		).toThrow('sync.invalid_key');
		const { nonce: _nonce, ...missing } = value;
		expect(() => parseKeyEnvelope(missing)).toThrow('sync.invalid_key');
		expect(() => parseKeyEnvelope({ ...value, extra: true })).toThrow(
			'sync.invalid_key',
		);
		expect(() =>
			parseKeyEnvelope({
				...ageEnvelope(keys.privateKey),
				recipientPublicKey: 'x',
			}),
		).toThrow('sync.invalid_key');
	});

	test('rejects base64 fields with the wrong decoded length', () => {
		const { keys } = signer();
		for (const changed of [
			{ recipientPublicKey: randomBytes(31).toString('base64') },
			{ ephemeralPublicKey: randomBytes(33).toString('base64') },
			{ salt: randomBytes(31).toString('base64') },
			{ nonce: randomBytes(13).toString('base64') },
			{ signature: randomBytes(63).toString('base64') },
			{ wrappedKey: 'not-base64!!' },
		])
			expect(() =>
				parseKeyEnvelope({ ...webEnvelope(keys.privateKey), ...changed }),
			).toThrow();
		const age = ageEnvelope(keys.privateKey);
		expect(() =>
			parseKeyEnvelope({
				...age,
				wrappedKey: randomBytes(20).toString('base64'),
			}),
		).toThrow();
	});
});
