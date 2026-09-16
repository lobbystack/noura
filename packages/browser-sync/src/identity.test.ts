import { describe, expect, test } from 'bun:test';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	createDeviceIdentity,
	createMemoryKeyStore,
	generateDeviceKeys,
	loadDeviceIdentity,
	openBundle,
	PBKDF2_MIN_ITERATIONS,
	sealBundle,
	sealIdentity,
	unlockDeviceIdentity,
} from './index';

const PASSPHRASE = 'correct horse battery staple';

describe('device identity custody', () => {
	test('generates a wrapped bundle and unlocks the same identity', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });

		expect(bundle.version).toBe(1);
		expect(bundle.kdf).toBe('pbkdf2-sha256');
		expect(bundle.iterations).toBeGreaterThanOrEqual(PBKDF2_MIN_ITERATIONS);
		expect(bundle.deviceId).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
		expect(bundle.ciphertext.length).toBeGreaterThan(0);

		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		expect(identity.deviceId).toBe(bundle.deviceId);
		expect(identity.signingPublic.length).toBe(32);
		expect(identity.x25519Public.length).toBe(32);
		expect(identity.signingSeed.length).toBe(32);
		expect(identity.x25519Secret.length).toBe(32);
		expect(identity.recipient.startsWith('x25519:')).toBe(true);
		expect(identity.token).toBe('');
	});

	test('sealBundle and openBundle round-trip raw key material and token', async () => {
		const keys = await generateDeviceKeys();
		const bundle = await sealBundle({
			passphrase: PASSPHRASE,
			deviceId: 'device_one',
			signingSeed: keys.signingSeed,
			x25519Secret: keys.x25519Secret,
			signingPublic: keys.signingPublic,
			x25519Public: keys.x25519Public,
			token: 'bearer-token-value',
		});

		expect(bundle.deviceId).toBe('device_one');
		expect(bundle.ciphertext).not.toContain('bearer-token-value');

		const identity = await openBundle(bundle, PASSPHRASE);
		expect(Array.from(identity.signingSeed)).toEqual(
			Array.from(keys.signingSeed),
		);
		expect(Array.from(identity.x25519Secret)).toEqual(
			Array.from(keys.x25519Secret),
		);
		expect(identity.token).toBe('bearer-token-value');
	});

	test('rejects a wrong passphrase with a structured error', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		let error: unknown;
		try {
			await openBundle(bundle, 'definitely not the passphrase');
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.PassphraseRejected,
		);
	});

	test('rejects a bundle below the PBKDF2 iteration minimum', async () => {
		await expect(
			createDeviceIdentity({ passphrase: PASSPHRASE, iterations: 1000 }),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('persists only the wrapped bundle in a key store', async () => {
		const keyStore = createMemoryKeyStore();
		const bundle = await createDeviceIdentity({
			passphrase: PASSPHRASE,
			keyStore,
			bundleId: 'bundle_one',
		});
		expect((await keyStore.read('bundle_one'))?.ciphertext).toBe(
			bundle.ciphertext,
		);

		const identity = await loadDeviceIdentity(
			keyStore,
			'bundle_one',
			PASSPHRASE,
		);
		expect(identity.deviceId).toBe(bundle.deviceId);

		await keyStore.delete('bundle_one');
		await expect(
			loadDeviceIdentity(keyStore, 'bundle_one', PASSPHRASE),
		).rejects.toBeInstanceOf(BrowserSyncError);
	});

	test('reseals an identity after recording an enrollment token', async () => {
		const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const sealed = await sealIdentity(
			{ ...identity, token: 'enrollment-token' },
			PASSPHRASE,
			{ bundleId: bundle.id },
		);
		const reopened = await openBundle(sealed, PASSPHRASE);
		expect(reopened.token).toBe('enrollment-token');
		expect(Array.from(reopened.signingSeed)).toEqual(
			Array.from(identity.signingSeed),
		);
	});
});
