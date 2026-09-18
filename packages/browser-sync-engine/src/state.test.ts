import { describe, expect, test } from 'bun:test';
import {
	BrowserSyncEngineErrorCode,
	createEmptySyncState,
	createFileSystemSyncStateStore,
	DEFAULT_SYNC_STATE_PATH,
	SYNC_STATE_VERSION,
	validateSyncState,
	type SyncState,
	type SyncStateFileSystem,
	type SyncStateMigration,
} from './index';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function bytes(value: string): Uint8Array {
	return encoder.encode(value);
}

/** In-memory filesystem; it never touches OPFS and deliberately ignores maxBytes. */
class FakeFileSystem implements SyncStateFileSystem {
	readonly files = new Map<string, Uint8Array>();
	readonly writes: string[] = [];
	readonly removes: string[] = [];
	requestedMaxBytes: number[] = [];

	async read(path: string, maxBytes?: number): Promise<Uint8Array | null> {
		if (maxBytes !== undefined) this.requestedMaxBytes.push(maxBytes);
		const value = this.files.get(path);
		return value === undefined ? null : value.slice();
	}

	async write(path: string, value: Uint8Array): Promise<void> {
		this.files.set(path, value.slice());
		this.writes.push(path);
	}

	async remove(path: string): Promise<boolean> {
		this.removes.push(path);
		return this.files.delete(path);
	}
}

function storedOperation(operationId: string) {
	return {
		version: 1 as const,
		operationId,
		workspaceId: 'workspace',
		objectId: 'object',
		deviceId: 'device',
		epoch: 1,
		policyRevision: '1',
		nonce: 'bm9uY2U=',
		ciphertext: 'Y2lwaGVy',
		signature: 'c2ln',
	};
}

function populatedState(): SyncState {
	return {
		version: SYNC_STATE_VERSION,
		cursor: '42',
		pushedRevisions: { 'b.md': 'rev-b', 'a.md': 'rev-a' },
		knownPaths: ['a.md', 'b.md'],
		outbox: [storedOperation('operation_1')],
		conflicts: [
			{
				operationId: 'operation_1',
				objectId: 'object',
				path: 'a.md',
				reason: 'revision_mismatch',
				expectedRevision: 'expected',
				currentRevision: 'current',
				previousPath: null,
				detectedAt: 1234,
				operation: storedOperation('operation_1'),
			},
		],
	};
}

/** State as it was persisted before conflicts carried the encrypted operation. */
function legacyState(): Record<string, unknown> {
	const state = populatedState() as unknown as Record<string, unknown>;
	const conflicts = (state.conflicts as Array<Record<string, unknown>>).map(
		({ operation: _operation, objectId: _objectId, ...legacy }) => legacy,
	);
	state.conflicts = conflicts;
	delete state.version;
	return state;
}

describe('createFileSystemSyncStateStore', () => {
	test('round-trips a populated state', async () => {
		const fileSystem = new FakeFileSystem();
		const store = createFileSystemSyncStateStore(
			fileSystem,
			'adapter/state.json',
		);

		await store.write(populatedState());
		expect(await store.read()).toEqual(populatedState());
		expect(fileSystem.writes).toEqual(['adapter/state.json']);
	});

	test('returns an empty state when the file is absent', async () => {
		const store = createFileSystemSyncStateStore(new FakeFileSystem());
		expect(await store.read()).toEqual(createEmptySyncState());
	});

	test('uses the adapter-owned default path outside the workspace', async () => {
		const fileSystem = new FakeFileSystem();
		const store = createFileSystemSyncStateStore(fileSystem);

		await store.write(createEmptySyncState());
		expect(fileSystem.writes).toEqual([DEFAULT_SYNC_STATE_PATH]);
		expect(DEFAULT_SYNC_STATE_PATH.startsWith('.noura/')).toBe(false);
	});

	test('reports corrupt JSON as a typed error without overwriting it', async () => {
		const fileSystem = new FakeFileSystem();
		const path = 'adapter/state.json';
		fileSystem.files.set(path, bytes('{ not json'));
		const store = createFileSystemSyncStateStore(fileSystem, path);

		await expect(store.read()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
		expect(decoder.decode(fileSystem.files.get(path)!)).toBe('{ not json');

		await expect(store.write(createEmptySyncState())).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
		expect(decoder.decode(fileSystem.files.get(path)!)).toBe('{ not json');
		expect(fileSystem.writes).toEqual([]);
	});

	test('reports schema-invalid JSON as a typed error', async () => {
		const fileSystem = new FakeFileSystem();
		fileSystem.files.set('state.json', bytes('{"cursor":5}'));
		const store = createFileSystemSyncStateStore(fileSystem, 'state.json');

		await expect(store.read()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
	});

	test('write resolves only after the bytes reach the filesystem', async () => {
		const fileSystem = new FakeFileSystem();
		const store = createFileSystemSyncStateStore(fileSystem, 'state.json');

		await store.write(populatedState());

		expect(fileSystem.files.has('state.json')).toBe(true);
		expect(await store.read()).toEqual(populatedState());
	});

	test('serializes deterministically regardless of key insertion order', async () => {
		const fileSystem = new FakeFileSystem();
		const first = createFileSystemSyncStateStore(fileSystem, 'first.json');
		const second = createFileSystemSyncStateStore(fileSystem, 'second.json');
		const state = populatedState();

		await first.write(state);
		await second.write({
			...state,
			pushedRevisions: { 'a.md': 'rev-a', 'b.md': 'rev-b' },
		});

		expect(fileSystem.files.get('first.json')).toEqual(
			fileSystem.files.get('second.json')!,
		);
	});

	test('bounds the read size and refuses to write an oversized state', async () => {
		const fileSystem = new FakeFileSystem();
		const store = createFileSystemSyncStateStore(fileSystem, 'state.json', 16);

		await expect(store.write(populatedState())).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
		expect(fileSystem.writes).toEqual([]);
		expect(fileSystem.requestedMaxBytes).toContain(16);
	});

	test('reports an oversized existing file as corrupt rather than parsing it', async () => {
		const fileSystem = new FakeFileSystem();
		const path = 'state.json';
		fileSystem.files.set(path, bytes('x'.repeat(64)));
		const store = createFileSystemSyncStateStore(fileSystem, path, 16);

		await expect(store.read()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
		await expect(store.write(createEmptySyncState())).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
		expect(fileSystem.writes).toEqual([]);
	});

	test('removes the backing file', async () => {
		const fileSystem = new FakeFileSystem();
		const store = createFileSystemSyncStateStore(fileSystem, 'state.json');

		await store.write(populatedState());
		expect(await store.remove()).toBe(true);
		expect(await store.read()).toEqual(createEmptySyncState());
		expect(await store.remove()).toBe(false);
	});
});

describe('sync state migration', () => {
	test('createEmptySyncState carries the current schema version', () => {
		expect(createEmptySyncState().version).toBe(SYNC_STATE_VERSION);
	});

	test('upgrades legacy state and drops unresolvable conflicts, reporting it', () => {
		const migrations: SyncStateMigration[] = [];
		const state = validateSyncState(legacyState(), {
			onMigration: (migration) => migrations.push(migration),
		});

		expect(state.version).toBe(SYNC_STATE_VERSION);
		expect(state.cursor).toBe('42');
		expect(state.conflicts).toEqual([]);
		expect(state.outbox).toHaveLength(1);
		expect(migrations).toEqual([
			{
				fromVersion: 1,
				toVersion: SYNC_STATE_VERSION,
				droppedConflicts: 1,
			},
		]);
	});

	test('a legacy state file reads without crashing and reports the migration', async () => {
		const fileSystem = new FakeFileSystem();
		const migrations: SyncStateMigration[] = [];
		fileSystem.files.set('state.json', bytes(JSON.stringify(legacyState())));
		const store = createFileSystemSyncStateStore(
			fileSystem,
			'state.json',
			undefined,
			{ onMigration: (migration) => migrations.push(migration) },
		);

		const state = await store.read();

		expect(state.version).toBe(SYNC_STATE_VERSION);
		expect(state.conflicts).toEqual([]);
		expect(migrations).toHaveLength(1);
		expect(migrations[0]!.droppedConflicts).toBe(1);
	});

	test('rejects state written by a newer schema version', () => {
		const state = { ...populatedState(), version: SYNC_STATE_VERSION + 1 };
		let caught: unknown;
		try {
			validateSyncState(state);
		} catch (error) {
			caught = error;
		}
		expect(caught).toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
	});

	test('rejects a conflict record that is not an object', () => {
		const state = populatedState() as unknown as Record<string, unknown>;
		state.conflicts = ['not-a-conflict'];
		let caught: unknown;
		try {
			validateSyncState(state);
		} catch (error) {
			caught = error;
		}
		expect(caught).toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidState,
		});
	});
});
