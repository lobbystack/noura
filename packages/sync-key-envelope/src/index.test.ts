import { describe, expect, test } from 'bun:test';
import deviceFixture from '../../../docs/workspace-format/fixtures/browser-device-v1.json';
import fixture from '../../../docs/workspace-format/fixtures/browser-key-v1.json';
import {
	decodeRecipient,
	deviceFingerprint,
	deviceFingerprintAge,
	deviceFingerprintForCard,
	encodeRecipient,
	enrollmentProof,
	KeyEnvelopeError,
	KeyEnvelopeErrorCode,
	unwrapKey,
	verifyEnvelope,
	wrapKey,
	type WebKeyEnvelope,
	type WebKeyWrapInputs,
} from './index';

interface ValidVector {
	name: string;
	inputs: WebKeyWrapInputs;
	trusted_signer_public: string;
	recipient_secret: string;
	expected_envelope: WebKeyEnvelope;
}

interface InvalidVector {
	name: string;
	operation: string;
	inputs?: WebKeyWrapInputs;
	trusted_signer_public?: string;
	recipient_secret?: string;
	envelope?: WebKeyEnvelope;
	expected_error: KeyEnvelopeErrorCode;
}

interface Fixtures {
	domain: string;
	version: number;
	info: string;
	object_key_domain: string;
	valid: ValidVector[];
	invalid: InvalidVector[];
}

const fixtures = fixture as unknown as Fixtures;

function decodeBase64(value: string): Uint8Array<ArrayBuffer> {
	return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

describe('browser-key-v1 fixtures', () => {
	test('fixture metadata matches exported constants', async () => {
		const { DOMAIN, VERSION, INFO, OBJECT_KEY_DOMAIN } =
			await import('./index');
		expect(fixtures.domain).toBe(DOMAIN);
		expect(fixtures.version).toBe(VERSION);
		expect(fixtures.info).toBe(new TextDecoder().decode(INFO));
		expect(fixtures.object_key_domain).toBe(OBJECT_KEY_DOMAIN);
		expect(fixtures.valid.length).toBeGreaterThan(0);
		expect(fixtures.invalid.length).toBeGreaterThan(0);
	});

	for (const vector of fixtures.valid) {
		test(`valid: ${vector.name}`, async () => {
			const envelope = await wrapKey(vector.inputs);
			expect(envelope).toEqual(vector.expected_envelope);

			const trusted = decodeBase64(vector.trusted_signer_public);
			await verifyEnvelope(envelope, trusted);

			const unwrapped = await unwrapKey(
				envelope,
				decodeBase64(vector.recipient_secret),
				trusted,
			);
			expect(Array.from(unwrapped)).toEqual(
				Array.from(decodeBase64(vector.inputs.object_key)),
			);
		});
	}

	for (const vector of fixtures.invalid) {
		test(`invalid: ${vector.name} (${vector.operation})`, async () => {
			let error: unknown;
			try {
				switch (vector.operation) {
					case 'wrap':
						await wrapKey(vector.inputs!);
						break;
					case 'verify':
						await verifyEnvelope(
							vector.envelope!,
							decodeBase64(vector.trusted_signer_public!),
						);
						break;
					case 'unwrap':
						await unwrapKey(
							vector.envelope!,
							decodeBase64(vector.recipient_secret!),
							decodeBase64(vector.trusted_signer_public!),
						);
						break;
					default:
						throw new Error(`unknown fixture operation ${vector.operation}`);
				}
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(KeyEnvelopeError);
			expect((error as KeyEnvelopeError).code).toBe(vector.expected_error);
			expect(Object.values(KeyEnvelopeErrorCode)).toContain(
				vector.expected_error,
			);
		});
	}
});

interface RecipientVector {
	name: string;
	public_key: string;
	recipient: string;
}

interface InvalidRecipientVector {
	name: string;
	recipient: string;
	expected_error: KeyEnvelopeErrorCode;
}

interface FingerprintVector {
	name: string;
	device_id: string;
	account_id: string;
	signing_public: string;
	recipient: string;
	expected_hex: string;
}

interface EnrollmentVector {
	name: string;
	origin: string;
	account_id: string;
	device_id: string;
	signing_secret: string;
	signing_public: string;
	recipient: string;
	challenge: string;
	expected_base64: string;
	expected_hex: string;
}

interface DeviceFixtures {
	recipient_prefix: string;
	recipients: {
		valid: RecipientVector[];
		invalid: InvalidRecipientVector[];
	};
	fingerprint: {
		domain: string;
		version: number;
		vectors: FingerprintVector[];
	};
	enrollment: {
		domain: string;
		version: number;
		vectors: EnrollmentVector[];
	};
}

const deviceFixtures = deviceFixture as unknown as DeviceFixtures;

function encodeBase64(value: Uint8Array): string {
	let binary = '';
	for (const byte of value) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function bytesToHex(value: Uint8Array): string {
	return Array.from(value, (byte) => byte.toString(16).padStart(2, '0')).join(
		'',
	);
}

describe('browser-device-v1 fixtures', () => {
	test('fixture metadata matches exported constants', async () => {
		const { RECIPIENT_PREFIX, DEVICE_FINGERPRINT_DOMAIN, ENROLLMENT_DOMAIN } =
			await import('./index');
		expect(deviceFixtures.recipient_prefix).toBe(RECIPIENT_PREFIX);
		expect(deviceFixtures.fingerprint.domain).toBe(DEVICE_FINGERPRINT_DOMAIN);
		expect(deviceFixtures.fingerprint.version).toBe(1);
		expect(deviceFixtures.enrollment.domain).toBe(ENROLLMENT_DOMAIN);
		expect(deviceFixtures.enrollment.version).toBe(1);
	});

	for (const vector of deviceFixtures.recipients.valid) {
		test(`recipient: ${vector.name}`, () => {
			const publicKey = decodeBase64(vector.public_key);
			expect(encodeRecipient(publicKey)).toBe(vector.recipient);
			expect(Array.from(decodeRecipient(vector.recipient))).toEqual(
				Array.from(publicKey),
			);
		});
	}

	for (const vector of deviceFixtures.recipients.invalid) {
		test(`invalid recipient: ${vector.name}`, () => {
			let error: unknown;
			try {
				decodeRecipient(vector.recipient);
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(KeyEnvelopeError);
			expect((error as KeyEnvelopeError).code).toBe(vector.expected_error);
		});
	}

	for (const vector of deviceFixtures.fingerprint.vectors) {
		test(`fingerprint: ${vector.name}`, () => {
			expect(
				deviceFingerprint(
					vector.device_id,
					vector.account_id,
					decodeBase64(vector.signing_public),
					vector.recipient,
				),
			).toBe(vector.expected_hex);
		});
	}

	for (const vector of deviceFixtures.enrollment.vectors) {
		test(`enrollment proof: ${vector.name}`, async () => {
			const proof = await enrollmentProof({
				origin: vector.origin,
				account_id: vector.account_id,
				device_id: vector.device_id,
				signing_secret: vector.signing_secret,
				signing_public: vector.signing_public,
				recipient: vector.recipient,
				challenge: vector.challenge,
			});
			expect(encodeBase64(proof)).toBe(vector.expected_base64);
			expect(bytesToHex(proof)).toBe(vector.expected_hex);
		});
	}
});

describe('device fingerprint dispatcher', () => {
	// The native vector is the exact Rust `device_fingerprint` output for the
	// `noura.device.card` tuple, verified against `crates/local-core`.
	const nativeVector = {
		deviceId: 'device_native',
		accountId: 'account_owner',
		signingPublic: 'iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=',
		recipient: 'age1qurswpc8qurswpc8qurswpc8qurswpc8qurswpc8qurswpc8qurs95jt69',
		expectedHex:
			'b310cf915085a5e71ec8bbcad7b6a67380fd485b7b38c8ea256b9ec18e6c0ffe',
	};

	test('routes browser recipients to the web fingerprint from the shared fixture', () => {
		for (const vector of deviceFixtures.fingerprint.vectors) {
			const signing = decodeBase64(vector.signing_public);
			expect(
				deviceFingerprintForCard(
					vector.device_id,
					vector.account_id,
					signing,
					vector.recipient,
				),
			).toBe(vector.expected_hex);
		}
	});

	test('routes native age recipients to the noura.device.card fingerprint', () => {
		const signing = decodeBase64(nativeVector.signingPublic);
		expect(
			deviceFingerprintAge(
				nativeVector.deviceId,
				nativeVector.accountId,
				signing,
				nativeVector.recipient,
			),
		).toBe(nativeVector.expectedHex);
		expect(
			deviceFingerprintForCard(
				nativeVector.deviceId,
				nativeVector.accountId,
				signing,
				nativeVector.recipient,
			),
		).toBe(nativeVector.expectedHex);
	});

	test('rejects a recipient that is neither x25519: nor age1', () => {
		let error: unknown;
		try {
			deviceFingerprintForCard(
				'device_x',
				'account_x',
				decodeBase64(nativeVector.signingPublic),
				'not-a-recipient',
			);
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(KeyEnvelopeError);
		expect((error as KeyEnvelopeError).code).toBe(
			KeyEnvelopeErrorCode.InvalidRecipient,
		);
	});
});
