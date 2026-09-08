import { expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import type { EncryptedPresence } from '../../../packages/shared/src/generated/EncryptedPresence';
import {
	MAX_REALTIME_BYTES,
	MAX_REALTIME_MESSAGES,
	RealtimeQueue,
	encryptedPresence,
	presenceSigningBytes,
	verifyPresence,
} from './realtime';

function fixture(): { value: EncryptedPresence; publicKey: string } {
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	const value: EncryptedPresence = {
		version: 1,
		workspaceId: 'workspace',
		objectId: 'object',
		generation: 'generation',
		epoch: 2,
		deviceId: 'device',
		sessionId: 'session',
		sequence: 1,
		nonce: randomBytes(12).toString('base64'),
		ciphertext: randomBytes(64).toString('base64'),
		signature: '',
	};
	value.signature = sign(
		null,
		presenceSigningBytes(value),
		keys.privateKey,
	).toString('base64');
	return { value, publicKey };
}

test('presence parser and signature bind every routing field', () => {
	const { value, publicKey } = fixture();
	const parsed = encryptedPresence(value);
	verifyPresence(parsed, publicKey);
	for (const changed of [
		{ objectId: 'other' },
		{ generation: 'other' },
		{ epoch: 3 },
		{ sessionId: 'other' },
		{ sequence: 2 },
	])
		expect(() =>
			verifyPresence(encryptedPresence({ ...value, ...changed }), publicKey),
		).toThrow('sync.invalid_signature');
	expect(() =>
		encryptedPresence({ ...value, selection: 'plaintext' }),
	).toThrow();
});

test('outbound realtime queues coalesce keys and enforce message capacity', () => {
	const sent: string[] = [];
	let overflow = 0;
	const queue = new RealtimeQueue(
		(value) => sent.push(value),
		() => overflow++,
	);
	queue.enqueue('changed', { type: 'changed', sequence: 1 });
	queue.enqueue('changed', { type: 'changed', sequence: 2 });
	for (let index = 0; index < MAX_REALTIME_MESSAGES - 1; index++)
		queue.enqueue(`presence:${index}`, { index });
	queue.enqueue('one-too-many', { nope: true });
	expect(overflow).toBe(1);
	queue.flush();
	expect(sent).toHaveLength(MAX_REALTIME_MESSAGES);
	expect(JSON.parse(sent[0]!)).toEqual({ type: 'changed', sequence: 2 });
});

test('outbound realtime queues reject aggregate byte overflow without flushing partial state', () => {
	const sent: string[] = [];
	let overflow = 0;
	const queue = new RealtimeQueue(
		(value) => sent.push(value),
		() => overflow++,
	);
	queue.enqueue('first', { ciphertext: 'a'.repeat(MAX_REALTIME_BYTES / 2) });
	queue.enqueue('second', { ciphertext: 'b'.repeat(MAX_REALTIME_BYTES / 2) });
	expect(overflow).toBe(1);
	queue.flush();
	expect(sent).toHaveLength(1);
});
