import { describe, expect, test } from 'bun:test';
import nativeFixture from '../../../docs/workspace-format/fixtures/native-recovery-v1.json';
import browserRecoveryFixture from '../../../docs/workspace-format/fixtures/browser-recovery-v1.json';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	MAX_RECOVERY_KIT_CIPHERTEXT_BYTES,
	PBKDF2_MIN_ITERATIONS,
	RECOVERY_KIT_FORMAT,
	RECOVERY_KIT_VERSION,
	createDeviceIdentity,
	decodeBase64,
	encodeBase64,
	exportRecoveryKit,
	importNativeRecoveryKit,
	importRecoveryKit,
	randomBytes,
	recoverNativeKeysToBrowserBinding,
	unlockDeviceIdentity,
	unwrapKey,
	type BrowserSyncBindingRecord,
	type NativeRecoveryObject,
	type WrappedKeyBundle,
} from './index';

const PASSPHRASE = 'correct horse battery staple';
const OTHER_PASSPHRASE = 'definitely not the passphrase';

interface NativeFixture {
	recovery: NativeRecoveryObject;
	recovery_identity: string;
	recovery_recipient: string;
	object_keys: { object_id: string; epoch: number; key: string }[];
}

const NATIVE = nativeFixture as unknown as NativeFixture;
const NATIVE_RECOVERY = NATIVE.recovery;
const NATIVE_RECOVERY_SIGNER = decodeBase64(
	NATIVE_RECOVERY.config.trustedDevices[NATIVE_RECOVERY.config.deviceId]!,
	32,
);

function bindingFor(bundle: WrappedKeyBundle): BrowserSyncBindingRecord {
	return {
		version: 1,
		localWorkspaceId: 'workspace_local',
		workspaceId: 'ws_remote',
		revision: '1',
		objectId: 'obj_one',
		objects: {
			obj_one: {
				path: 'notes/a.md',
				localObjectId: 'note_a',
				epoch: 1,
				policyRevision: '1',
				key: {
					deviceId: bundle.deviceId,
					wrappedKey: encodeBase64(randomBytes(48)),
					signature: encodeBase64(randomBytes(64)),
					construction: 'web',
					recipientPublicKey: encodeBase64(randomBytes(32)),
					ephemeralPublicKey: encodeBase64(randomBytes(32)),
					salt: encodeBase64(randomBytes(32)),
					nonce: encodeBase64(randomBytes(12)),
				},
			},
		},
		pinnedSigners: { [bundle.deviceId]: encodeBase64(randomBytes(32)) },
	};
}

async function fixture() {
	const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
	const binding = bindingFor(bundle);
	return { bundle, binding };
}

function flipLastByte(value: string): string {
	const bytes = decodeBase64(value);
	bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0x01;
	return encodeBase64(bytes);
}

describe('browser recovery kit', () => {
	test('round-trips a wrapped bundle and binding', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});

		expect(kit.format).toBe(RECOVERY_KIT_FORMAT);
		expect(kit.version).toBe(RECOVERY_KIT_VERSION);
		expect(kit.kdf).toBe('pbkdf2-sha256');
		expect(kit.iterations).toBeGreaterThanOrEqual(PBKDF2_MIN_ITERATIONS);
		expect(decodeBase64(kit.salt, 32)).toHaveLength(32);
		expect(decodeBase64(kit.nonce, 12)).toHaveLength(12);
		expect(Number.isNaN(Date.parse(kit.createdAt))).toBe(false);

		const recovered = await importRecoveryKit(kit, PASSPHRASE);
		expect(recovered.bundle).toEqual(bundle);
		expect(recovered.binding).toEqual(binding);
	});

	test('rejects a wrong passphrase with a typed error', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});
		let error: unknown;
		try {
			await importRecoveryKit(kit, OTHER_PASSPHRASE);
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.PassphraseRejected,
		);
	});

	test('rejects tampered ciphertext', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});
		const tampered = { ...kit, ciphertext: flipLastByte(kit.ciphertext) };
		await expect(
			importRecoveryKit(tampered, PASSPHRASE),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('rejects an unknown version and an unknown format, never guessing', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});

		let versionError: unknown;
		try {
			await importRecoveryKit({ ...kit, version: 2 }, PASSPHRASE);
		} catch (caught) {
			versionError = caught;
		}
		expect((versionError as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
		);

		let formatError: unknown;
		try {
			await importRecoveryKit(
				{ ...kit, format: 'noura.other-kit' },
				PASSPHRASE,
			);
		} catch (caught) {
			formatError = caught;
		}
		expect(formatError).toBeInstanceOf(BrowserSyncError);
		expect((formatError as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.InvalidBundle,
		);
	});

	test('rejects oversized input before decrypting', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});
		const tooLarge = {
			...kit,
			ciphertext: 'A'.repeat(
				Math.ceil((MAX_RECOVERY_KIT_CIPHERTEXT_BYTES * 4) / 3) + 64,
			),
		};
		await expect(
			importRecoveryKit(tooLarge, PASSPHRASE),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('exports no plaintext key material', async () => {
		const { bundle, binding } = await fixture();
		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});
		const serialized = JSON.stringify(kit);
		for (const secret of [
			PASSPHRASE,
			'signingSeed',
			'x25519Secret',
			bundle.ciphertext,
			binding.objects.obj_one!.key.wrappedKey,
			binding.pinnedSigners[bundle.deviceId]!,
		]) {
			expect(serialized).not.toContain(secret);
		}
	});

	test('rejects an empty passphrase and below-minimum iterations', async () => {
		const { bundle, binding } = await fixture();
		await expect(
			exportRecoveryKit({ bundle, binding, passphrase: '' }),
		).rejects.toBeInstanceOf(BrowserSyncError);
		await expect(
			exportRecoveryKit({
				bundle,
				binding,
				passphrase: PASSPHRASE,
				iterations: PBKDF2_MIN_ITERATIONS - 1,
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);

		const kit = await exportRecoveryKit({
			bundle,
			binding,
			passphrase: PASSPHRASE,
		});
		await expect(importRecoveryKit(kit, '')).rejects.toBeInstanceOf(
			BrowserSyncError,
		);
	});
});

describe('native recovery interoperability', () => {
	test('recovers the fixture object keys through the pinned recovery signer', async () => {
		const result = await importNativeRecoveryKit({
			recovery: NATIVE_RECOVERY,
			recoveryIdentity: NATIVE.recovery_identity,
			trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
		});
		expect(result.entries.map((entry) => entry.objectId)).toEqual([
			'object_one',
			'object_two',
		]);
		for (const vector of NATIVE.object_keys) {
			const recovered = result.keys.get(vector.object_id);
			expect(recovered).toBeDefined();
			expect(encodeBase64(recovered!)).toBe(vector.key);
		}
		expect(result.entries[0]!.epoch).toBe(1);
		expect(result.entries[1]!.epoch).toBe(2);
	});

	test('rejects a wrong recovery identity because the identity is signed', async () => {
		const wrong = `${NATIVE.recovery_identity.slice(0, -1)}4`;
		let error: unknown;
		try {
			await importNativeRecoveryKit({
				recovery: NATIVE_RECOVERY,
				recoveryIdentity: wrong,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			});
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.InvalidSignature,
		);
	});

	test('rejects a tampered recovery signature', async () => {
		const tampered: NativeRecoveryObject = {
			...NATIVE_RECOVERY,
			signature: flipLastByte(NATIVE_RECOVERY.signature),
		};
		await expect(
			importNativeRecoveryKit({
				recovery: tampered,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('rejects a tampered envelope even with a valid recovery signature shape', async () => {
		const tampered: NativeRecoveryObject = {
			...NATIVE_RECOVERY,
			envelopes: NATIVE_RECOVERY.envelopes.map((envelope, index) =>
				index === 0
					? { ...envelope, wrappedKey: flipLastByte(envelope.wrappedKey) }
					: envelope,
			),
		};
		let error: unknown;
		try {
			await importNativeRecoveryKit({
				recovery: tampered,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			});
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.InvalidSignature,
		);
	});

	test('rejects an unpinned envelope signer and a mismatched envelope recipient', async () => {
		const unpinned: NativeRecoveryObject = {
			...NATIVE_RECOVERY,
			config: { ...NATIVE_RECOVERY.config, trustedDevices: {} },
		};
		await expect(
			importNativeRecoveryKit({
				recovery: unpinned,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);

		const wrongDevice: NativeRecoveryObject = {
			...NATIVE_RECOVERY,
			envelopes: NATIVE_RECOVERY.envelopes.map((envelope) => ({
				...envelope,
				deviceId: 'device_other',
			})),
		};
		await expect(
			importNativeRecoveryKit({
				recovery: wrongDevice,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('re-wraps a recovered key to the browser device as a noura.sync.key.web envelope', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const device = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const result = await recoverNativeKeysToBrowserBinding({
			recovery: NATIVE_RECOVERY,
			recoveryIdentity: NATIVE.recovery_identity,
			trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			device,
			localWorkspaceId: 'workspace_local',
			revision: '1',
			objectId: 'object_one',
			objects: {
				object_one: {
					path: 'notes/one.md',
					localObjectId: 'note_one',
					epoch: 1,
					policyRevision: '1',
				},
			},
		});

		const bound = result.binding.objects.object_one!.key;
		expect(bound.construction).toBe('web');
		expect(bound.deviceId).toBe(device.deviceId);

		const unwrapped = await unwrapKey(
			{
				workspace_id: NATIVE_RECOVERY.config.workspaceId,
				object_id: 'object_one',
				epoch: 1,
				signing_device: bound.deviceId,
				device_id: bound.deviceId,
				recipient_public_key: bound.recipientPublicKey,
				ephemeral_public_key: bound.ephemeralPublicKey,
				salt: bound.salt,
				nonce: bound.nonce,
				wrapped_key: bound.wrappedKey,
				signature: bound.signature,
			},
			device.x25519Secret,
			device.signingPublic,
		);
		const expected = NATIVE.object_keys.find(
			(vector) => vector.object_id === 'object_one',
		)!;
		expect(encodeBase64(unwrapped)).toBe(expected.key);

		const serialized = JSON.stringify(result.binding);
		expect(serialized).not.toContain(NATIVE.recovery_identity);
		expect(serialized).not.toContain(expected.key);
	});

	test('marks recovered objects unmapped so they never own local files', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const device = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const result = await recoverNativeKeysToBrowserBinding({
			recovery: NATIVE_RECOVERY,
			recoveryIdentity: NATIVE.recovery_identity,
			trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
			device,
			localWorkspaceId: 'workspace_local',
			revision: '1',
			objectId: 'object_one',
			objects: {
				object_one: {
					path: 'notes/one.md',
					epoch: 1,
					policyRevision: '1',
				},
			},
		});
		expect(result.binding.objects.object_one?.unmapped).toBe(true);
	});

	test('rejects a binding object id that could be read as a file path', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const device = await unlockDeviceIdentity(bundle, PASSPHRASE);
		await expect(
			recoverNativeKeysToBrowserBinding({
				recovery: NATIVE_RECOVERY,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
				device,
				localWorkspaceId: 'workspace_local',
				revision: '1',
				objectId: 'notes/victim.md',
				objects: {
					'notes/victim.md': {
						path: 'notes/victim.md',
						epoch: 1,
						policyRevision: '1',
					},
				},
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('rejects a requested epoch the recovery object does not carry', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const device = await unlockDeviceIdentity(bundle, PASSPHRASE);
		await expect(
			recoverNativeKeysToBrowserBinding({
				recovery: NATIVE_RECOVERY,
				recoveryIdentity: NATIVE.recovery_identity,
				trustedRecoverySigner: NATIVE_RECOVERY_SIGNER,
				device,
				localWorkspaceId: 'workspace_local',
				revision: '1',
				objectId: 'object_one',
				objects: {
					object_one: {
						path: 'notes/one.md',
						epoch: 9,
						policyRevision: '1',
					},
				},
			}),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('the shared browser recovery fixture unlocks and recovers the expected keys', async () => {
		const fixture = browserRecoveryFixture as {
			kit: unknown;
			recovery_passphrase: string;
			device_passphrase: string;
			expected: {
				device_id: string;
				object_keys: Array<{ object_id: string; epoch: number; key: string }>;
			};
		};
		const { bundle, binding } = await importRecoveryKit(
			fixture.kit,
			fixture.recovery_passphrase,
		);
		const identity = await unlockDeviceIdentity(
			bundle,
			fixture.device_passphrase,
		);
		expect(identity.deviceId).toBe(fixture.expected.device_id);

		const recovered = new Map<string, string>();
		for (const [objectId, bound] of Object.entries(binding.objects)) {
			const key = await unwrapKey(
				{
					workspace_id: binding.workspaceId,
					object_id: objectId,
					epoch: bound.epoch,
					signing_device: bound.key.deviceId,
					device_id: bound.key.deviceId,
					recipient_public_key: bound.key.recipientPublicKey,
					ephemeral_public_key: bound.key.ephemeralPublicKey,
					salt: bound.key.salt,
					nonce: bound.key.nonce,
					wrapped_key: bound.key.wrappedKey,
					signature: bound.key.signature,
				},
				identity.x25519Secret,
				identity.signingPublic,
			);
			recovered.set(objectId, encodeBase64(key));
		}
		for (const expected of fixture.expected.object_keys) {
			expect(recovered.get(expected.object_id)).toBe(expected.key);
		}
		await expect(
			importRecoveryKit(fixture.kit, OTHER_PASSPHRASE),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});
});
