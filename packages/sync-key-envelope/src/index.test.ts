import { describe, expect, test } from 'bun:test';
import fixture from '../../../docs/workspace-format/fixtures/browser-key-v1.json';
import {
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
