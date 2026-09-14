import { expect, test } from 'bun:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import {
	accessDigest,
	accessPolicy,
	accessSigningBytes,
	keySigningBytes,
	verifyAccess,
	type AccessPolicy,
	type AccessPolicyEnvelope,
} from './access';
import { browserKeySigningBytes, type BrowserKeyTupleFields } from './browser';

const workspace = 'workspace';
const object = 'object';
const policyDevice = 'device_signer';

function signer() {
	const keys = generateKeyPairSync('ed25519');
	const publicKey = keys.publicKey
		.export({ format: 'der', type: 'spki' })
		.subarray(-32)
		.toString('base64');
	return { keys, publicKey };
}

function webEnvelope(
	privateKey: Parameters<typeof sign>[2],
	overrides: Partial<BrowserKeyTupleFields> = {},
): AccessPolicyEnvelope {
	const fields: BrowserKeyTupleFields = {
		workspaceId: workspace,
		objectId: object,
		epoch: 1,
		signingDevice: policyDevice,
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
		construction: 'web',
		recipientPublicKey: fields.recipientPublicKey,
		ephemeralPublicKey: fields.ephemeralPublicKey,
		salt: fields.salt,
		nonce: fields.nonce,
	};
}

function ageEnvelope(
	privateKey: Parameters<typeof sign>[2],
): AccessPolicyEnvelope {
	const envelope = {
		deviceId: 'device_age',
		wrappedKey: randomBytes(80).toString('base64'),
	};
	return {
		...envelope,
		signature: sign(
			null,
			keySigningBytes(workspace, object, 1, policyDevice, envelope),
			privateKey,
		).toString('base64'),
	};
}

function value(
	envelopes: AccessPolicyEnvelope[],
	version: 1 | 2 = 1,
): Omit<AccessPolicy, 'signature'> {
	return {
		version,
		workspaceId: workspace,
		revision: '1',
		previousPolicyDigest: null,
		deviceId: policyDevice,
		members: [{ accountId: 'account_owner', role: 'owner' }],
		objects: [{ objectId: object, epoch: 1, grants: [], envelopes }],
	};
}

function signed(
	body: Omit<AccessPolicy, 'signature'>,
	privateKey: Parameters<typeof sign>[2],
): AccessPolicy {
	return accessPolicy({
		...body,
		signature: sign(null, accessSigningBytes(body), privateKey).toString(
			'base64',
		),
	});
}

test('browser policy envelopes parse, verify and bind every browser field', () => {
	const { keys, publicKey } = signer();
	const body = value([webEnvelope(keys.privateKey)]);
	const policy = signed(body, keys.privateKey);
	expect(policy.objects[0]!.envelopes[0]!.construction).toBe('web');
	expect(() => verifyAccess(policy, publicKey)).not.toThrow();
	// A changed browser field invalidates the policy signature and its digest.
	const tampered = signed(
		value([
			{
				...webEnvelope(keys.privateKey),
				nonce: randomBytes(12).toString('base64'),
			},
		]),
		keys.privateKey,
	);
	expect(accessDigest(tampered)).not.toBe(accessDigest(policy));
	// A swapped per-envelope signature is rejected under a valid policy signature.
	const swapped = signed(
		value([
			{
				...webEnvelope(keys.privateKey),
				signature: randomBytes(64).toString('base64'),
			},
		]),
		keys.privateKey,
	);
	expect(() => verifyAccess(swapped, publicKey)).toThrow(
		'sync.invalid_signature',
	);
});

test('browser policy envelopes reject unknown constructions and wrong shapes', () => {
	const { keys } = signer();
	const envelope = webEnvelope(keys.privateKey);
	expect(() =>
		accessPolicy({
			...value([envelope]),
			signature: randomBytes(64).toString('base64'),
		}),
	).not.toThrow();
	for (const malformed of [
		{ ...envelope, construction: 'wasm' },
		{ ...envelope, extra: true },
	]) {
		expect(() =>
			accessPolicy({
				...value([malformed as unknown as AccessPolicyEnvelope]),
				signature: randomBytes(64).toString('base64'),
			}),
		).toThrow('sync.invalid_policy');
	}
	expect(() =>
		accessPolicy({
			...value([{ ...envelope, nonce: randomBytes(13).toString('base64') }]),
			signature: randomBytes(64).toString('base64'),
		}),
	).toThrow();
	const { nonce: _nonce, ...missing } = envelope;
	expect(() =>
		accessPolicy({
			...value([missing as AccessPolicyEnvelope]),
			signature: randomBytes(64).toString('base64'),
		}),
	).toThrow('sync.invalid_policy');
});

test('native age policy envelopes verify exactly as before', () => {
	const { keys, publicKey } = signer();
	const policy = signed(value([ageEnvelope(keys.privateKey)]), keys.privateKey);
	expect(policy.objects[0]!.envelopes[0]!.construction).toBe('age');
	expect(() => verifyAccess(policy, publicKey)).not.toThrow();
	const tampered = structuredClone(policy);
	tampered.objects[0]!.envelopes[0]!.wrappedKey =
		randomBytes(80).toString('base64');
	expect(() => verifyAccess(tampered, publicKey)).toThrow(
		'sync.invalid_signature',
	);
});
