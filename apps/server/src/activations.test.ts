import { expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import type { ObjectActivation } from '../../../packages/shared/src/generated/ObjectActivation';
import type { WorkspaceCapability } from '../../../packages/shared/src/generated/WorkspaceCapability';
import { keySigningBytes } from './access';
import {
	activationDigest,
	activationSigningBytes,
	objectActivation,
	verifyObjectActivation,
} from './activations';
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
	return { activation, capability, publicKey };
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
