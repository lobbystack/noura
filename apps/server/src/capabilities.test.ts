import { expect, test } from 'bun:test';
import { generateKeyPairSync, sign } from 'node:crypto';
import {
	capabilitySigningBytes,
	verifyWorkspaceCapability,
	workspaceCapability,
} from './capabilities';

function fixture() {
	const { privateKey, publicKey } = generateKeyPairSync('ed25519');
	const rawPublicKey = publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	const value = workspaceCapability({
		version: 1,
		workspaceId: 'workspace',
		collaborationVersion: 1,
		minimumClientVersion: 1,
		minimumRelayVersion: 1,
		deviceId: 'device',
		signature: Buffer.alloc(64).toString('base64'),
	});
	value.signature = sign(
		null,
		capabilitySigningBytes(value),
		privateKey,
	).toString('base64');
	return { value, rawPublicKey };
}

test('workspace capability binds its workspace, versions, and owner device', () => {
	const { value, rawPublicKey } = fixture();
	verifyWorkspaceCapability(value, rawPublicKey);
	for (const field of ['workspaceId', 'deviceId'] as const) {
		const changed = { ...value, [field]: 'changed' };
		expect(() => verifyWorkspaceCapability(changed, rawPublicKey)).toThrow(
			'sync.invalid_signature',
		);
	}
});

test('workspace capability rejects unknown fields and incompatible versions', () => {
	const { value } = fixture();
	expect(() => workspaceCapability({ ...value, plaintext: 'secret' })).toThrow(
		'sync.invalid_collaboration_capability',
	);
	expect(() =>
		workspaceCapability({ ...value, minimumClientVersion: 2 }),
	).toThrow('sync.incompatible_collaboration_capability');
});
