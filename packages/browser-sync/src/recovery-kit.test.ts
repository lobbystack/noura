import { describe, expect, test } from 'bun:test';
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
	importRecoveryKit,
	randomBytes,
	type BrowserSyncBindingRecord,
	type WrappedKeyBundle,
} from './index';

const PASSPHRASE = 'correct horse battery staple';
const OTHER_PASSPHRASE = 'definitely not the passphrase';

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
