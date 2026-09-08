import { createPublicKey, verify } from 'node:crypto';
import type { EncryptedPresence } from '../../../packages/shared/src/generated/EncryptedPresence';
import { base64, identifier, record, SyncError } from './protocol';

const MAX_SAFE_INTEGER = 9_007_199_254_740_991;
export const MAX_REALTIME_MESSAGES = 256;
export const MAX_REALTIME_BYTES = 1024 * 1024;

function exact(input: unknown, fields: string[]) {
	const value = record(input);
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_presence');
	return value;
}

export function encryptedPresence(input: unknown): EncryptedPresence {
	const value = exact(input, [
		'version',
		'workspaceId',
		'objectId',
		'generation',
		'epoch',
		'deviceId',
		'sessionId',
		'sequence',
		'nonce',
		'ciphertext',
		'signature',
	]);
	if (
		value.version !== 1 ||
		!Number.isSafeInteger(value.epoch) ||
		(value.epoch as number) < 1 ||
		(value.epoch as number) > MAX_SAFE_INTEGER ||
		!Number.isSafeInteger(value.sequence) ||
		(value.sequence as number) < 1 ||
		(value.sequence as number) > MAX_SAFE_INTEGER
	)
		throw new SyncError('sync.invalid_presence');
	return {
		version: 1,
		workspaceId: identifier(value.workspaceId),
		objectId: identifier(value.objectId),
		generation: identifier(value.generation),
		epoch: value.epoch as number,
		deviceId: identifier(value.deviceId),
		sessionId: identifier(value.sessionId),
		sequence: value.sequence as number,
		nonce: base64(value.nonce, 12).toString('base64'),
		ciphertext: base64(value.ciphertext, 16, 4096).toString('base64'),
		signature: base64(value.signature, 64).toString('base64'),
	};
}

export function presenceSigningBytes(value: EncryptedPresence) {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.presence',
			value.version,
			value.workspaceId,
			value.objectId,
			value.generation,
			value.epoch,
			value.deviceId,
			value.sessionId,
			value.sequence,
			value.nonce,
			value.ciphertext,
		]),
	);
}

export function verifyPresence(value: EncryptedPresence, publicKey: string) {
	const key = createPublicKey({
		key: Buffer.concat([
			Buffer.from('302a300506032b6570032100', 'hex'),
			base64(publicKey, 32),
		]),
		format: 'der',
		type: 'spki',
	});
	if (
		!verify(null, presenceSigningBytes(value), key, base64(value.signature, 64))
	)
		throw new SyncError('sync.invalid_signature', 403);
}

/** Coalesces transient notifications before touching the runtime socket queue. */
export class RealtimeQueue {
	private values = new Map<string, string>();
	private bytes = 0;
	constructor(
		private send: (value: string) => void,
		private overflow: () => void,
	) {}
	enqueue(key: string, value: unknown) {
		const encoded = JSON.stringify(value);
		const bytes = Buffer.byteLength(encoded);
		if (bytes > MAX_REALTIME_BYTES) return this.overflow();
		const prior = this.values.get(key);
		const nextBytes =
			this.bytes - (prior ? Buffer.byteLength(prior) : 0) + bytes;
		if (
			(!prior && this.values.size >= MAX_REALTIME_MESSAGES) ||
			nextBytes > MAX_REALTIME_BYTES
		)
			return this.overflow();
		this.values.set(key, encoded);
		this.bytes = nextBytes;
	}
	flush() {
		const values = [...this.values.values()];
		this.values.clear();
		this.bytes = 0;
		for (const value of values) this.send(value);
	}
}
