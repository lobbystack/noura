import { expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import activationFixture from '../../../docs/workspace-format/fixtures/activation-v1.json';
import type { ObjectActivation } from '../../../packages/shared/src/generated/ObjectActivation';
import type { WorkspaceCapability } from '../../../packages/shared/src/generated/WorkspaceCapability';
import { keySigningBytes } from './access';
import {
	activationDigest,
	activationSigningBytes,
	objectActivation,
	verifyActivationRecipients,
	verifyObjectActivation,
} from './activations';
import { browserKeySigningBytes, type BrowserKeyTupleFields } from './browser';
import { capabilityDigest, capabilitySigningBytes } from './capabilities';
import { checkpointSigningBytes } from './checkpoints';
import { signingBytes } from './protocol';

function fixture() {
	const workspaceId = 'workspace';
	const objectId = 'object';
	const deviceId = 'device';
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	const capability: WorkspaceCapability = {
		version: 1,
		workspaceId,
		collaborationVersion: 1,
		minimumClientVersion: 1,
		minimumRelayVersion: 1,
		deviceId,
		signature: '',
	};
	capability.signature = sign(
		null,
		capabilitySigningBytes(capability),
		keys.privateKey,
	).toString('base64');
	const operation = {
		version: 1 as const,
		operationId: crypto.randomUUID(),
		workspaceId,
		objectId,
		deviceId,
		epoch: 1,
		policyRevision: '7',
		nonce: randomBytes(12).toString('base64'),
		ciphertext: randomBytes(40).toString('base64'),
		signature: '',
	};
	operation.signature = sign(
		null,
		signingBytes(operation),
		keys.privateKey,
	).toString('base64');
	const checkpoint = {
		version: 1 as const,
		generation: 'generation',
		coveredSequence: '12',
		payload: operation,
		signature: '',
	};
	checkpoint.signature = sign(
		null,
		checkpointSigningBytes(checkpoint),
		keys.privateKey,
	).toString('base64');
	const envelope = {
		deviceId,
		wrappedKey: randomBytes(60).toString('base64'),
		signature: '',
	};
	envelope.signature = sign(
		null,
		keySigningBytes(workspaceId, objectId, 1, deviceId, envelope),
		keys.privateKey,
	).toString('base64');
	const activation: ObjectActivation = {
		version: 1,
		activationId: crypto.randomUUID(),
		workspaceId,
		policyRevision: '7',
		coveredSequence: '12',
		capabilityDigest: capabilityDigest(capability),
		deviceId,
		document: { generation: 'generation', mode: 'text' },
		envelopes: [envelope],
		checkpoint,
		signature: '',
	};
	activation.signature = sign(
		null,
		activationSigningBytes(activation),
		keys.privateKey,
	).toString('base64');
	return { activation, capability, publicKey, keys };
}

function webEnvelope(
	privateKey: Parameters<typeof sign>[2],
	overrides: Partial<BrowserKeyTupleFields> = {},
) {
	const fields: BrowserKeyTupleFields = {
		workspaceId: 'workspace',
		objectId: 'object',
		epoch: 1,
		signingDevice: 'device',
		deviceId: 'device_browser',
		recipientPublicKey: randomBytes(32).toString('base64'),
		ephemeralPublicKey: randomBytes(32).toString('base64'),
		salt: randomBytes(32).toString('base64'),
		nonce: randomBytes(12).toString('base64'),
		wrappedKey: randomBytes(80).toString('base64'),
		...overrides,
	};
	return {
		deviceId: fields.deviceId,
		wrappedKey: fields.wrappedKey,
		signature: sign(null, browserKeySigningBytes(fields), privateKey).toString(
			'base64',
		),
		construction: 'web' as const,
		recipientPublicKey: fields.recipientPublicKey,
		ephemeralPublicKey: fields.ephemeralPublicKey,
		salt: fields.salt,
		nonce: fields.nonce,
	};
}

function withEnvelopes(
	base: ReturnType<typeof fixture>,
	envelopes: ObjectActivation['envelopes'],
): ObjectActivation {
	const activation: ObjectActivation = { ...base.activation, envelopes };
	activation.signature = sign(
		null,
		activationSigningBytes(activation),
		base.keys.privateKey,
	).toString('base64');
	return activation;
}

test('object activation binds capability, current boundary, checkpoint and recipients', () => {
	const { activation, capability, publicKey } = fixture();
	const parsed = objectActivation(activation);
	verifyObjectActivation(parsed, publicKey, capability);
	expect(activationDigest(parsed)).toMatch(/^[0-9a-f]{64}$/);
	for (const changed of [
		{ policyRevision: '8' },
		{ coveredSequence: '13' },
		{ capabilityDigest: '0'.repeat(64) },
	]) {
		expect(() =>
			verifyObjectActivation(
				objectActivation({ ...activation, ...changed }),
				publicKey,
				capability,
			),
		).toThrow();
	}
});

test('object activation rejects duplicate recipients, unknown fields and mismatched blobs', () => {
	const { activation } = fixture();
	expect(() =>
		objectActivation({
			...activation,
			envelopes: [activation.envelopes[0], activation.envelopes[0]],
		}),
	).toThrow();
	expect(() =>
		objectActivation({ ...activation, plaintext: 'secret' }),
	).toThrow();
	expect(() =>
		objectActivation({
			...activation,
			version: 2,
			blobs: [
				{
					objectId: 'other',
					epoch: 1,
					ciphertextDigest: 'a'.repeat(64),
					ciphertextSize: 10,
				},
			],
		}),
	).toThrow();
});

test('object activation accepts a valid browser envelope and signs over the web tuple', () => {
	const base = fixture();
	const web = webEnvelope(base.keys.privateKey);
	const activation = withEnvelopes(base, [base.activation.envelopes[0]!, web]);
	const parsed = objectActivation(activation);
	expect(parsed.envelopes[1]!.construction).toBe('web');
	expect(() =>
		verifyObjectActivation(parsed, base.publicKey, base.capability),
	).not.toThrow();

	// The activation signing tuple covers every browser field, so the signature
	// is only valid while those bytes are unchanged.
	const baseBytes = activationSigningBytes(parsed);
	const tampered = objectActivation({
		...activation,
		envelopes: [
			base.activation.envelopes[0]!,
			{ ...web, salt: randomBytes(32).toString('base64') },
		],
	});
	expect(activationSigningBytes(tampered)).not.toEqual(baseBytes);
});

test('object activation rejects unknown constructions and mixed or missing browser fields', () => {
	const base = fixture();
	const web = webEnvelope(base.keys.privateKey);
	const age = base.activation.envelopes[0]!;
	const activation = withEnvelopes(base, [age, web]);
	for (const malformed of [
		[{ ...web, construction: 'wasm' }, age],
		[age, { ...web, extra: true }],
		[age, { ...web, recipientPublicKey: undefined }],
	]) {
		expect(() =>
			objectActivation({
				...activation,
				envelopes: malformed as unknown as ObjectActivation['envelopes'],
			}),
		).toThrow('sync.invalid_object_activation');
	}
	const { nonce: _nonce, ...missing } = web;
	expect(() =>
		objectActivation({
			...activation,
			envelopes: [age, missing as ObjectActivation['envelopes'][number]],
		}),
	).toThrow('sync.invalid_object_activation');
});

test('object activation binds a browser envelope to the enrolled device recipient', () => {
	const base = fixture();
	const web = webEnvelope(base.keys.privateKey);
	const activation = withEnvelopes(base, [base.activation.envelopes[0]!, web]);
	const parsed = objectActivation(activation);
	const enrolled = `x25519:${web.recipientPublicKey}`;
	expect(() =>
		verifyActivationRecipients(
			parsed.envelopes,
			new Map([['device_browser', enrolled]]),
		),
	).not.toThrow();
	expect(() =>
		verifyActivationRecipients(
			parsed.envelopes,
			new Map([
				['device_browser', `x25519:${randomBytes(32).toString('base64')}`],
			]),
		),
	).toThrow('sync.invalid_recipient');
	expect(() => verifyActivationRecipients(parsed.envelopes, new Map())).toThrow(
		'sync.invalid_recipient',
	);
});

test('activation signing bytes match the shared browser fixture', () => {
	const vector = activationFixture.vectors[0]!;
	const parsed = objectActivation(vector.activation);
	expect(activationSigningBytes(parsed).toString()).toBe(
		vector.expected_signing_bytes,
	);
});
