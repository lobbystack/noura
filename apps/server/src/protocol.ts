import { createHash, createPublicKey, verify } from 'node:crypto';
import type { EncryptedOperation } from '../../../packages/shared/src/sync';

export class SyncError extends Error {
	constructor(
		public readonly code: string,
		public readonly status = 400,
	) {
		super(code);
	}
}

export function identifier(value: unknown): string {
	if (typeof value !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(value)) {
		throw new SyncError('sync.invalid_id');
	}
	return value;
}

export function cursor(value: unknown): string {
	if (
		typeof value !== 'string' ||
		!/^(0|[1-9][0-9]{0,18})$/.test(value) ||
		BigInt(value) > 9223372036854775807n
	) {
		throw new SyncError('sync.invalid_cursor');
	}
	return value;
}

export function base64(value: unknown, min: number, max = min): Buffer {
	if (typeof value !== 'string' || value.length > Math.ceil(max / 3) * 4) {
		throw new SyncError('sync.invalid_encoding');
	}
	const decoded = Buffer.from(value, 'base64');
	if (
		decoded.toString('base64') !== value ||
		decoded.length < min ||
		decoded.length > max
	) {
		throw new SyncError('sync.invalid_encoding');
	}
	return decoded;
}

export function record(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value))
		throw new SyncError('sync.invalid_request');
	return value as Record<string, unknown>;
}

export function operation(value: unknown): EncryptedOperation {
	const input = record(value);
	const keys = [
		'version',
		'operationId',
		'workspaceId',
		'objectId',
		'deviceId',
		'epoch',
		'policyRevision',
		'nonce',
		'ciphertext',
		'signature',
	];
	if (input.version === 2) keys.push('generation', 'kind');
	if (
		Object.keys(input).length !== keys.length ||
		keys.some((key) => !(key in input))
	)
		throw new SyncError('sync.invalid_envelope');
	if (input.version !== 1 && input.version !== 2)
		throw new SyncError('sync.unsupported_version');
	if (input.version === 2) {
		identifier(input.generation);
		if (!['text', 'metadata', 'file'].includes(input.kind as string))
			throw new SyncError('sync.invalid_operation_kind');
	}
	for (const key of ['operationId', 'workspaceId', 'objectId', 'deviceId'])
		identifier(input[key]);
	if (!Number.isSafeInteger(input.epoch) || (input.epoch as number) < 1)
		throw new SyncError('sync.invalid_epoch');
	cursor(input.policyRevision);
	base64(input.nonce, 12);
	base64(input.ciphertext, 16, 1024 * 1024);
	base64(input.signature, 64);
	return input as unknown as EncryptedOperation;
}

/** Fixed tuple gives Rust and TypeScript one unambiguous signing representation. */
export function signingBytes(
	op: Omit<EncryptedOperation, 'signature'>,
): Buffer {
	return Buffer.from(
		JSON.stringify([
			'noura.sync.operation',
			op.version,
			op.workspaceId,
			op.objectId,
			op.deviceId,
			op.operationId,
			op.epoch,
			op.policyRevision,
			op.nonce,
			op.ciphertext,
			...(op.version === 2 ? [op.generation, op.kind] : []),
		]),
	);
}

export function verifyOperation(
	op: EncryptedOperation,
	publicKey: string,
): void {
	const raw = base64(publicKey, 32);
	const key = createPublicKey({
		key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), raw]),
		format: 'der',
		type: 'spki',
	});
	if (!verify(null, signingBytes(op), key, base64(op.signature, 64)))
		throw new SyncError('sync.invalid_signature', 403);
}

export function digest(value: string | Uint8Array): string {
	return createHash('sha256').update(value).digest('hex');
}

export function operationDigest(op: EncryptedOperation): string {
	return digest(Buffer.concat([signingBytes(op), base64(op.signature, 64)]));
}
