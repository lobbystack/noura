import { describe, expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import {
	base64,
	cursor,
	operation,
	signingBytes,
	verifyOperation,
} from './protocol';
import type { EncryptedOperation } from '../../../packages/shared/src/sync';

export function fixture(
	deviceId = 'device_a',
	workspaceId = 'workspace_a',
	objectId = 'object_a',
) {
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	function make(
		id: string = crypto.randomUUID(),
		epoch = 1,
		policyRevision = '0',
	): EncryptedOperation {
		const body = {
			version: 1 as const,
			operationId: id,
			workspaceId,
			objectId,
			deviceId,
			epoch,
			policyRevision,
			nonce: randomBytes(12).toString('base64'),
			ciphertext: randomBytes(40).toString('base64'),
		};
		return {
			...body,
			signature: sign(null, signingBytes(body), keys.privateKey).toString(
				'base64',
			),
		};
	}
	return { publicKey, make, keys };
}

describe('opaque wire validation', () => {
	test('signatures bind ciphertext, workspace, object, device, policy, epoch and operation identity', () => {
		const f = fixture();
		const op = f.make();
		expect(() => verifyOperation(operation(op), f.publicKey)).not.toThrow();
		for (const changed of [
			{ workspaceId: 'other' },
			{ objectId: 'other' },
			{ deviceId: 'other' },
			{ operationId: 'other' },
			{ epoch: 2 },
			{ policyRevision: '1' },
			{ ciphertext: randomBytes(40).toString('base64') },
			{ nonce: randomBytes(12).toString('base64') },
		]) {
			expect(() => verifyOperation({ ...op, ...changed }, f.publicKey)).toThrow(
				'sync.invalid_signature',
			);
		}
	});
	test('unknown plaintext fields and unsupported versions are rejected', () => {
		const op = fixture().make();
		expect(() => operation({ ...op, path: 'private.md' })).toThrow(
			'sync.invalid_envelope',
		);
		expect(() => operation({ ...op, version: 2 })).toThrow(
			'sync.unsupported_version',
		);
	});
	test('base64 is canonical and bounded', () => {
		for (const value of ['!!!!', 'YQ', 'YQ==\n', 'YWJj'])
			expect(() => base64(value, 1)).toThrow();
		expect(base64('YQ==', 1).toString()).toBe('a');
	});
	test('cursors retain bigint precision and reject overflow', () => {
		expect(cursor('9007199254740993')).toBe('9007199254740993');
		for (const value of ['-1', '01', '1e3', '9223372036854775808', 1])
			expect(() => cursor(value)).toThrow();
	});
});
