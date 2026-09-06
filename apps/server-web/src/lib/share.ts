import type { EncryptedOperation } from '../../../../packages/shared/src/sync';

const maximum = 1024 * 1024;
const utf8 = new TextEncoder();
export type SharePayload = {
	version: 1;
	title: string;
	markdown: string;
	updatedAt: string;
};
export type ShareKeys = {
	key: Uint8Array<ArrayBuffer>;
	signer: Uint8Array<ArrayBuffer>;
};
export class ShareError extends Error {}
const invalid = () =>
	new ShareError(
		'This shared item could not be verified. Ask the owner for a new link.',
	);
function bytes(
	value: unknown,
	size: number,
	url = false,
	exact = true,
): Uint8Array<ArrayBuffer> {
	if (typeof value !== 'string' || value.length > Math.ceil(size / 3) * 4)
		throw invalid();
	const encoded = url
		? value.replace(/-/g, '+').replace(/_/g, '/') +
			'='.repeat((4 - (value.length % 4)) % 4)
		: value;
	let decoded: string;
	try {
		decoded = atob(encoded);
	} catch {
		throw invalid();
	}
	if (decoded.length > size || (exact && decoded.length !== size))
		throw invalid();
	const canonical = url
		? btoa(decoded).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
		: btoa(decoded);
	if (value !== canonical) throw invalid();
	return Uint8Array.from(decoded, (char) => char.charCodeAt(0));
}
export function shareKeys(fragment: string): ShareKeys {
	const values = new URLSearchParams(fragment.replace(/^#/, ''));
	if (values.size !== 2 || !values.has('key') || !values.has('signer'))
		throw new ShareError(
			'This link is missing its decryption key. Ask the owner to copy the complete share link.',
		);
	return {
		key: bytes(values.get('key'), 32, true),
		signer: bytes(values.get('signer'), 32, true),
	};
}
function operation(value: unknown): EncryptedOperation {
	if (!value || typeof value !== 'object' || Array.isArray(value))
		throw invalid();
	const op = value as EncryptedOperation;
	const fields = [
		'version',
		'workspaceId',
		'objectId',
		'deviceId',
		'operationId',
		'epoch',
		'policyRevision',
		'nonce',
		'ciphertext',
		'signature',
	];
	if (
		Object.keys(op).length !== fields.length ||
		Object.keys(op).some((key) => !fields.includes(key)) ||
		op.version !== 1 ||
		!Number.isSafeInteger(op.epoch) ||
		op.epoch < 1 ||
		typeof op.policyRevision !== 'string' ||
		!/^(0|[1-9][0-9]*)$/.test(op.policyRevision) ||
		BigInt(op.policyRevision) > 9223372036854775807n
	)
		throw invalid();
	for (const id of [op.workspaceId, op.objectId, op.deviceId, op.operationId])
		if (typeof id !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(id))
			throw invalid();
	return op;
}
export async function decryptShare(
	value: unknown,
	keys: ShareKeys,
): Promise<SharePayload> {
	if (!globalThis.crypto?.subtle)
		throw new ShareError(
			'This browser needs a secure connection and WebCrypto support to open encrypted shares.',
		);
	const op = operation(value);
	const nonce = bytes(op.nonce, 12);
	const ciphertext = bytes(op.ciphertext, maximum + 16, false, false);
	const signature = bytes(op.signature, 64);
	if (ciphertext.length < 16) throw invalid();
	const route = [
		1,
		op.workspaceId,
		op.objectId,
		op.deviceId,
		op.operationId,
		op.epoch,
		op.policyRevision,
	];
	let signingKey: CryptoKey;
	try {
		signingKey = await crypto.subtle.importKey(
			'raw',
			keys.signer,
			{ name: 'Ed25519' },
			false,
			['verify'],
		);
	} catch {
		throw new ShareError(
			'This browser does not support the encryption used by this link. Open it in a current browser.',
		);
	}
	try {
		const verified = await crypto.subtle.verify(
			'Ed25519',
			signingKey,
			signature,
			utf8.encode(
				JSON.stringify([
					'noura.sync.operation',
					...route,
					op.nonce,
					op.ciphertext,
				]),
			),
		);
		if (!verified) throw invalid();
		const aes = await crypto.subtle.importKey(
			'raw',
			keys.key,
			'AES-GCM',
			false,
			['decrypt'],
		);
		const clear = await crypto.subtle.decrypt(
			{
				name: 'AES-GCM',
				iv: nonce,
				additionalData: utf8.encode(
					JSON.stringify(['noura.sync.payload', ...route]),
				),
			},
			aes,
			ciphertext,
		);
		if (clear.byteLength > maximum) throw invalid();
		let payload: SharePayload;
		try {
			payload = JSON.parse(
				new TextDecoder('utf-8', { fatal: true }).decode(clear),
			);
		} finally {
			new Uint8Array(clear).fill(0);
		}
		if (
			!payload ||
			typeof payload !== 'object' ||
			Object.keys(payload).sort().join(',') !==
				'markdown,title,updatedAt,version' ||
			payload.version !== 1 ||
			typeof payload.title !== 'string' ||
			payload.title.length > 4096 ||
			typeof payload.markdown !== 'string' ||
			typeof payload.updatedAt !== 'string' ||
			!/^\d{4}-\d{2}-\d{2}T/.test(payload.updatedAt) ||
			!Number.isFinite(Date.parse(payload.updatedAt))
		)
			throw invalid();
		return payload;
	} catch {
		throw invalid();
	}
}
export async function loadShare(
	token: string,
	keys: ShareKeys,
	signal: AbortSignal,
): Promise<{ revision: string; payload: SharePayload }> {
	if (!/^[A-Za-z0-9_-]{43}$/.test(token)) throw invalid();
	const response = await fetch(`/public/${token}`, {
		signal: AbortSignal.any([signal, AbortSignal.timeout(10000)]),
		cache: 'no-store',
		credentials: 'omit',
		referrerPolicy: 'no-referrer',
	});
	if (response.status === 404)
		throw new ShareError(
			'This share link has expired or been revoked. Ask the owner for a new link.',
		);
	if (!response.ok || !response.body)
		throw new ShareError('Unable to check this shared item. Reconnecting…');
	const reader = response.body.getReader();
	let length = 0;
	const chunks: Uint8Array[] = [];
	try {
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			length += value.byteLength;
			if (length > 2 * maximum) throw invalid();
			chunks.push(value);
		}
	} finally {
		await reader.cancel();
		reader.releaseLock();
	}
	const result = new Uint8Array(length);
	let offset = 0;
	for (const chunk of chunks) {
		result.set(chunk, offset);
		offset += chunk.length;
	}
	let body: { revision: string; snapshot: unknown };
	try {
		body = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(result));
	} catch {
		throw invalid();
	}
	if (
		!body ||
		typeof body !== 'object' ||
		typeof body.revision !== 'string' ||
		!/^[1-9][0-9]{0,18}$/.test(body.revision) ||
		BigInt(body.revision) > 9223372036854775807n
	)
		throw invalid();
	return {
		revision: body.revision,
		payload: await decryptShare(body.snapshot, keys),
	};
}
