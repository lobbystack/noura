import { describe, expect, test } from 'bun:test';
import nativeFixture from '../../../docs/workspace-format/fixtures/native-recovery-v1.json';
import {
	createMemoryBindingStore,
	createMemoryKeyStore,
	decodeBase64,
	encodeBase64,
	type NativeRecoveryObject,
} from '@noura/browser-sync';
import { createMemorySyncStateStore } from '@noura/browser-sync-engine';
import {
	createBrowserSyncController,
	DEFAULT_BROWSER_SYNC_BUNDLE_ID,
} from './controller';
import {
	bootstrapPerObject,
	createSyncServerState,
	encoder,
	enrollingFetch,
	ORIGIN,
	PASSPHRASE,
	recordingSyncFetch,
	workspaceFiles,
} from './test-support';

describe('browser recovery kit controller wiring', () => {
	const OTHER_PASSPHRASE = 'definitely not the passphrase';

	test('export requires unlocked custody and a loaded binding', async () => {
		const unavailable = await createBrowserSyncController({ keyStore: null });
		const unavailableResult = await unavailable.exportRecoveryKit(PASSPHRASE);
		expect(unavailableResult.ok).toBe(false);
		if (!unavailableResult.ok)
			expect(unavailableResult.code).toBe('unavailable');

		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: enrollingFetch(),
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
		});
		const locked = await controller.exportRecoveryKit(PASSPHRASE);
		expect(locked.ok).toBe(false);
		if (!locked.ok) expect(locked.code).toBe('locked');

		const emptyPassphrase = await controller.exportRecoveryKit('');
		expect(emptyPassphrase.ok).toBe(false);
		if (!emptyPassphrase.ok)
			expect(emptyPassphrase.code).toBe('invalid_passphrase');

		await controller.enroll({ passphrase: PASSPHRASE });
		const noBinding = await controller.exportRecoveryKit(PASSPHRASE);
		expect(noBinding.ok).toBe(false);
		if (!noBinding.ok) expect(noBinding.code).toBe('not_configured');
	});

	test('import restores a usable binding on another browser without a trusted device', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const source = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await source.controller.exportRecoveryKit(PASSPHRASE);
		expect(exported.ok).toBe(true);
		if (!exported.ok) throw new Error(exported.message);

		const targetKeyStore = createMemoryKeyStore();
		const targetBindingStore = createMemoryBindingStore();
		const targetStateStore = createMemorySyncStateStore();
		const target = await createBrowserSyncController({
			keyStore: targetKeyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore: targetBindingStore,
			stateStore: targetStateStore,
		});

		const imported = await target.importRecoveryKit(
			exported.value,
			PASSPHRASE,
			'workspace_restored',
		);
		expect(imported.ok).toBe(true);
		if (!imported.ok) throw new Error(imported.message);
		expect(imported.value.configured).toBe(true);
		expect(imported.value.workspaceId).toBe(source.record.workspaceId);

		const stored = await targetBindingStore.read();
		expect(stored?.localWorkspaceId).toBe('workspace_restored');
		expect(stored?.workspaceId).toBe(source.record.workspaceId);
		expect(stored?.objects[source.record.objectId]?.key.wrappedKey).toBe(
			source.record.objects[source.record.objectId]?.key.wrappedKey,
		);

		const unlocked = await target.unlock({ passphrase: PASSPHRASE });
		expect(unlocked.ok).toBe(true);
		target.setWorkspaceFiles(files);
		const bound = await target.bindWorkspace({
			workspaceId: 'workspace_restored',
			workspaceFiles: files,
		});
		expect(bound.ok).toBe(true);
		if (bound.ok) expect(bound.value.configured).toBe(true);
	});

	test('import rejects a wrong passphrase and writes nothing', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const source = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await source.controller.exportRecoveryKit(PASSPHRASE);
		if (!exported.ok) throw new Error(exported.message);

		const targetKeyStore = createMemoryKeyStore();
		const targetBindingStore = createMemoryBindingStore();
		const target = await createBrowserSyncController({
			keyStore: targetKeyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore: targetBindingStore,
			stateStore: createMemorySyncStateStore(),
		});

		const imported = await target.importRecoveryKit(
			exported.value,
			OTHER_PASSPHRASE,
			'workspace_restored',
		);
		expect(imported.ok).toBe(false);
		if (!imported.ok) expect(imported.code).toBe('passphrase_rejected');
		expect(
			await targetKeyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID),
		).toBeUndefined();
		expect(await targetBindingStore.read()).toBeNull();
	});

	test('the exported kit contains no plaintext key material', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await controller.exportRecoveryKit(PASSPHRASE);
		if (!exported.ok) throw new Error(exported.message);

		const serialized = JSON.stringify(exported.value);
		expect(serialized).not.toContain(PASSPHRASE);
		expect(serialized).not.toContain('signingSeed');
		expect(serialized).not.toContain('x25519Secret');
	});
});

interface NativeRecoveryFixture {
	recovery: NativeRecoveryObject;
	recovery_identity: string;
	recovery_recipient: string;
	object_keys: Array<{ object_id: string; epoch: number; key: string }>;
}

const NATIVE_KIT = nativeFixture as unknown as NativeRecoveryFixture;

function nativeFlipLastByte(value: string): string {
	const bytes = decodeBase64(value);
	bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0x01;
	return encodeBase64(bytes);
}

function changedIdentity(identity: string): string {
	const last = identity.at(-1);
	return `${identity.slice(0, -1)}${last === '3' ? '4' : '3'}`;
}

describe('browser native recovery kit import', () => {
	async function nativeController() {
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const keyStore = createMemoryKeyStore();
		const controller = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: enrollingFetch('token-native'),
			bindingStore,
			stateStore,
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		return { controller, keyStore, bindingStore, stateStore };
	}

	test('imports the shared fixture and persists only re-wrapped keys', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);
		if (!result.ok) throw new Error(result.message);
		expect(result.value.configured).toBe(true);

		const record = await bindingStore.read();
		expect(record).not.toBeNull();
		expect(record?.localWorkspaceId).toBe('workspace_native');
		expect(record?.workspaceId).toBe(NATIVE_KIT.recovery.config.workspaceId);
		expect(record?.revision).toBe('1');
		expect(Object.keys(record!.objects).sort()).toEqual([
			'object_one',
			'object_two',
		]);
		expect(record!.objectId in record!.objects).toBe(true);

		const deviceId = controller.device()?.deviceId;
		expect(deviceId).toBeDefined();
		for (const bound of Object.values(record!.objects)) {
			expect(bound.key.construction).toBe('web');
			expect(bound.key.deviceId).toBe(deviceId!);
			expect(bound.key.wrappedKey.length).toBeGreaterThan(0);
			// A native kit carries no verified paths, so recovered objects must
			// never be treated as owners of local files.
			expect(bound.unmapped).toBe(true);
		}
	});

	test('never writes the recovery identity or a plaintext object key', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);

		const serialized = JSON.stringify(await bindingStore.read());
		expect(serialized).not.toContain(NATIVE_KIT.recovery_identity);
		expect(serialized).not.toContain('recovery_identity');
		expect(serialized).not.toContain('AGE-SECRET-KEY');
		for (const vector of NATIVE_KIT.object_keys) {
			expect(serialized).not.toContain(vector.key);
		}
	});

	test('rejects replacing a binding that points at a different remote workspace', async () => {
		const { controller, bindingStore } = await nativeController();
		const boundKey = {
			deviceId: 'device_existing',
			wrappedKey: '',
			signature: '',
			construction: 'web' as const,
			recipientPublicKey: '',
			ephemeralPublicKey: '',
			salt: '',
			nonce: '',
		};
		await bindingStore.write({
			version: 1,
			localWorkspaceId: 'workspace_native',
			workspaceId: 'workspace_other',
			revision: '1',
			objectId: 'object_other',
			objects: {
				object_other: {
					path: 'notes/other.md',
					epoch: 1,
					policyRevision: '1',
					key: boundKey,
				},
			},
			pinnedSigners: { device_existing: '' },
		});
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		// The existing binding must be left untouched.
		expect((await bindingStore.read())?.workspaceId).toBe('workspace_other');
	});

	test('accepts a caller-pinned signer matching the kit self-description', async () => {
		const { controller, bindingStore } = await nativeController();
		const signer =
			NATIVE_KIT.recovery.config.trustedDevices[
				NATIVE_KIT.recovery.config.deviceId
			]!;
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			recoverySignerPublic: signer,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);
		expect(await bindingStore.read()).not.toBeNull();
	});

	test('rejects a pinned signer that disagrees with the kit and writes nothing', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			recoverySignerPublic: encodeBase64(new Uint8Array(32).fill(7)),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a recovery identity that does not match the embedded one', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: changedIdentity(NATIVE_KIT.recovery_identity),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a tampered kit and writes nothing', async () => {
		const { controller, bindingStore } = await nativeController();

		const tamperedSignature = {
			...NATIVE_KIT,
			recovery: {
				...NATIVE_KIT.recovery,
				signature: nativeFlipLastByte(NATIVE_KIT.recovery.signature),
			},
		};
		const signatureResult = await controller.importNativeRecoveryKit(
			tamperedSignature,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(signatureResult.ok).toBe(false);
		if (!signatureResult.ok)
			expect(signatureResult.code).toBe('custody_failed');

		const first = NATIVE_KIT.recovery.envelopes[0]!;
		const tamperedEnvelope = {
			...NATIVE_KIT,
			recovery: {
				...NATIVE_KIT.recovery,
				envelopes: [
					{ ...first, wrappedKey: nativeFlipLastByte(first.wrappedKey) },
					...NATIVE_KIT.recovery.envelopes.slice(1),
				],
			},
		};
		const envelopeResult = await controller.importNativeRecoveryKit(
			tamperedEnvelope,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(envelopeResult.ok).toBe(false);
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a wrong identity when the kit does not embed one', async () => {
		const { controller, bindingStore } = await nativeController();
		const { recovery_identity: _omitted, ...withoutIdentity } = NATIVE_KIT;
		const result = await controller.importNativeRecoveryKit(withoutIdentity, {
			recoveryIdentity: changedIdentity(NATIVE_KIT.recovery_identity),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('returns a typed locked or unavailable result when custody is missing', async () => {
		const unavailable = await createBrowserSyncController({ keyStore: null });
		const unavailableResult = await unavailable.importNativeRecoveryKit(
			NATIVE_KIT,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(unavailableResult.ok).toBe(false);
		if (!unavailableResult.ok)
			expect(unavailableResult.code).toBe('unavailable');

		const locked = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: enrollingFetch(),
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
		});
		const lockedResult = await locked.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(lockedResult.ok).toBe(false);
		if (!lockedResult.ok) expect(lockedResult.code).toBe('locked');
	});
});
