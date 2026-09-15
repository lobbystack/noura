import { describe, expect, test } from 'bun:test';
import type { EncryptedOperation } from '@noura/shared';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	createDeviceIdentity,
	createFileChangeCodec,
	encodeBase64,
	unlockDeviceIdentity,
	type DeviceIdentity,
	type FileChangeDescriptor,
} from './index';

const PASSPHRASE = 'correct horse battery staple';
const encoder = new TextEncoder();
const OBJECT_ID = 'object';
const OBJECT_KEY = new Uint8Array(32).fill(7);

async function makeIdentity(): Promise<DeviceIdentity> {
	const bundle = await createDeviceIdentity({ passphrase: PASSPHRASE });
	return unlockDeviceIdentity(bundle, PASSPHRASE);
}

function pinned(identity: DeviceIdentity): Map<string, string> {
	return new Map([[identity.deviceId, encodeBase64(identity.signingPublic)]]);
}

const CHANGE: FileChangeDescriptor = {
	path: 'notes/a.md',
	previousPath: null,
	baseRevision: null,
	content: encoder.encode('hello world'),
};

async function seal(
	identity: DeviceIdentity,
	change: FileChangeDescriptor = CHANGE,
): Promise<EncryptedOperation> {
	const codec = createFileChangeCodec({
		identity,
		objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
		pinnedSigners: pinned(identity),
	});
	return codec.sealFileChange({
		workspaceId: 'workspace',
		objectId: OBJECT_ID,
		epoch: 1,
		policyRevision: '1',
		change,
	});
}

describe('createFileChangeCodec', () => {
	test('seals and opens a file change round trip', async () => {
		const identity = await makeIdentity();
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
			pinnedSigners: pinned(identity),
		});
		const sealed = await codec.sealFileChange({
			workspaceId: 'workspace',
			objectId: OBJECT_ID,
			epoch: 1,
			policyRevision: '1',
			change: CHANGE,
		});
		expect(sealed.version).toBe(1);
		expect(sealed.deviceId).toBe(identity.deviceId);

		const opened = await codec.openFileChange(sealed);
		expect(opened.workspaceId).toBe('workspace');
		expect(opened.objectId).toBe(OBJECT_ID);
		expect(opened.epoch).toBe(1);
		expect(opened.version).toBe(1);
		expect(opened.path).toBe(CHANGE.path);
		expect(opened.previousPath).toBeNull();
		expect(opened.baseRevision).toBeNull();
		expect(Array.from(opened.content!)).toEqual(Array.from(CHANGE.content!));
	});

	test('round trips a deletion', async () => {
		const identity = await makeIdentity();
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
			pinnedSigners: pinned(identity),
		});
		const deletion: FileChangeDescriptor = {
			path: 'gone.md',
			previousPath: null,
			baseRevision: null,
			content: null,
		};
		const sealed = await codec.sealFileChange({
			workspaceId: 'workspace',
			objectId: OBJECT_ID,
			epoch: 1,
			policyRevision: '1',
			change: deletion,
		});
		const opened = await codec.openFileChange(sealed);
		expect(opened.content).toBeNull();
		expect(opened.path).toBe('gone.md');
	});

	test('rejects an unpinned signer', async () => {
		const identity = await makeIdentity();
		const sealed = await seal(identity);
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
			pinnedSigners: new Map(),
		});
		let error: unknown;
		try {
			await codec.openFileChange(sealed);
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.UntrustedSigner,
		);
	});

	test('rejects a malformed pinned signer key', async () => {
		const identity = await makeIdentity();
		const sealed = await seal(identity);
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
			pinnedSigners: new Map([[identity.deviceId, 'not base64!']]),
		});
		await expect(codec.openFileChange(sealed)).rejects.toMatchObject({
			code: BrowserSyncErrorCode.UntrustedSigner,
		});
	});

	test('rejects a tampered operation', async () => {
		const identity = await makeIdentity();
		const sealed = await seal(identity);
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map([[OBJECT_ID, OBJECT_KEY]]),
			pinnedSigners: pinned(identity),
		});
		const tampered: EncryptedOperation = {
			...sealed,
			signature: encodeBase64(new Uint8Array(64)),
		};
		await expect(codec.openFileChange(tampered)).rejects.toMatchObject({
			code: BrowserSyncErrorCode.InvalidOperationSignature,
		});
	});

	test('rejects a missing object key on open', async () => {
		const identity = await makeIdentity();
		const sealed = await seal(identity);
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map(),
			pinnedSigners: pinned(identity),
		});
		await expect(codec.openFileChange(sealed)).rejects.toMatchObject({
			code: BrowserSyncErrorCode.MissingKey,
		});
	});

	test('rejects a missing object key on seal', async () => {
		const identity = await makeIdentity();
		const codec = createFileChangeCodec({
			identity,
			objectKeys: new Map(),
			pinnedSigners: pinned(identity),
		});
		await expect(
			codec.sealFileChange({
				workspaceId: 'workspace',
				objectId: OBJECT_ID,
				epoch: 1,
				policyRevision: '1',
				change: CHANGE,
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.MissingKey });
	});
});
