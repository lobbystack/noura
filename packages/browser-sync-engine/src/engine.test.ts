import { describe, expect, test } from 'bun:test';
import type {
	EncryptedOperation,
	SequencedOperation,
	SyncPage,
} from '@noura/shared';
import fixture from '../../../docs/workspace-format/fixtures/sync-v1.json';
import {
	BrowserSyncEngine,
	BrowserSyncEngineErrorCode,
	cloneSyncState,
	createBrowserStorageAdapter,
	createEmptySyncState,
	MemorySyncStorage,
	memoryRevision,
	type BrowserSyncEngineOptions,
	type BrowserSyncRemote,
	type BrowserWorkspaceStorageLike,
	type FileChange,
	type FileChangeCodec,
	type SyncState,
	type SyncStateStore,
} from './index';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function bytes(value: string): Uint8Array {
	return encoder.encode(value);
}

function toBase64(input: Uint8Array): string {
	return Buffer.from(input).toString('base64');
}

function fromBase64(value: string): Uint8Array {
	return new Uint8Array(Buffer.from(value, 'base64'));
}

function canonicalFileChange(change: FileChange): string {
	return JSON.stringify({
		version: 1,
		path: change.path,
		previousPath: change.previousPath,
		baseRevision: change.baseRevision,
		content: change.content === null ? null : toBase64(change.content),
	});
}

let operationCounter = 0;

function encodeFileChange(change: FileChange): EncryptedOperation {
	operationCounter += 1;
	return {
		version: 1,
		operationId: `operation_${operationCounter}`,
		workspaceId: 'workspace',
		objectId: 'object',
		deviceId: 'device',
		epoch: 1,
		policyRevision: '1',
		nonce: toBase64(new Uint8Array(12)),
		ciphertext: toBase64(encoder.encode(canonicalFileChange(change))),
		signature: toBase64(new Uint8Array(64)),
	};
}

function openCanonical(operation: EncryptedOperation): FileChange & {
	workspaceId: string;
	objectId: string;
	epoch: number;
} {
	const payload = JSON.parse(
		decoder.decode(fromBase64(operation.ciphertext)),
	) as {
		path: string;
		previousPath: string | null;
		baseRevision: string | null;
		content: string | null;
	};
	return {
		workspaceId: operation.workspaceId,
		objectId: operation.objectId,
		epoch: operation.epoch,
		path: payload.path,
		previousPath: payload.previousPath,
		baseRevision: payload.baseRevision,
		content: payload.content === null ? null : fromBase64(payload.content),
	};
}

function createFakeCodec(): FileChangeCodec {
	return {
		async sealFileChange(change) {
			return encodeFileChange(change);
		},
		async openFileChange(operation) {
			return openCanonical(operation);
		},
	};
}

function revokedError(): Error {
	const error = new Error('revoked');
	(error as { code?: string }).code = 'revoked';
	return error;
}

class FakeRemote implements BrowserSyncRemote {
	batches: EncryptedOperation[][] = [];
	pages: SyncPage[] = [];
	failNextPush: Error | null = null;
	revokedOnPush = false;
	revokedOnPull = false;

	async push(
		operations: EncryptedOperation[],
	): Promise<{ sequences: string[] }> {
		if (this.revokedOnPush) throw revokedError();
		if (this.failNextPush !== null) {
			const error = this.failNextPush;
			this.failNextPush = null;
			throw error;
		}
		this.batches.push([...operations]);
		return {
			sequences: operations.map((_, index) =>
				String(this.batches.length * 1000 + index),
			),
		};
	}

	async pull(cursor: string): Promise<SyncPage> {
		if (this.revokedOnPull) throw revokedError();
		return (
			this.pages.shift() ?? {
				accessRevision: cursor,
				operations: [],
				cursor,
				hasMore: false,
			}
		);
	}
}

function change(
	path: string,
	content: string | null,
	options: { previousPath?: string; baseRevision?: string } = {},
): FileChange {
	return {
		path,
		previousPath: options.previousPath ?? null,
		baseRevision: options.baseRevision ?? null,
		content: content === null ? null : bytes(content),
	};
}

function page(
	changes: FileChange[],
	cursor: string,
	hasMore = false,
): SyncPage {
	return {
		accessRevision: cursor,
		cursor,
		hasMore,
		operations: changes.map((entry, index) => ({
			...encodeFileChange(entry),
			sequence: String(index),
		})) as SequencedOperation[],
	};
}

/** Durable store with a synchronous snapshot for assertions. */
function createTestStateStore(initial: SyncState = createEmptySyncState()): {
	store: SyncStateStore;
	snapshot(): SyncState;
} {
	let current = cloneSyncState(initial);
	return {
		store: {
			async read() {
				return cloneSyncState(current);
			},
			async write(next) {
				current = cloneSyncState(next);
			},
		},
		snapshot() {
			return cloneSyncState(current);
		},
	};
}

function createHarness(
	options: {
		storage?: MemorySyncStorage;
		codec?: FileChangeCodec;
		state?: SyncState;
		onRevoked?: BrowserSyncEngineOptions['onRevoked'];
	} = {},
): {
	engine: BrowserSyncEngine;
	storage: MemorySyncStorage;
	remote: FakeRemote;
	revoked: unknown[];
	persisted(): SyncState;
} {
	const storage = options.storage ?? new MemorySyncStorage();
	const remote = new FakeRemote();
	const codec = options.codec ?? createFakeCodec();
	const stateStore = createTestStateStore(options.state);
	const revoked: unknown[] = [];
	const onRevoked =
		options.onRevoked ??
		((error: unknown) => {
			revoked.push(error);
		});
	const engine = new BrowserSyncEngine({
		storage,
		remote,
		codec,
		state: stateStore.store,
		now: () => 1234,
		onRevoked,
	});
	return {
		engine,
		storage,
		remote,
		revoked,
		persisted: stateStore.snapshot,
	};
}

describe('outbox durability', () => {
	test('enqueueFileChange persists the outbox in order before any network call', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		await harness.storage.write({
			path: 'b.md',
			bytes: bytes('b'),
			expectedRevision: null,
		});

		const first = await harness.engine.enqueueFileChange(change('a.md', 'a2'));
		const second = await harness.engine.enqueueFileChange(change('b.md', 'b2'));

		expect(harness.persisted().outbox.map((op) => op.operationId)).toEqual([
			first.operationId,
			second.operationId,
		]);
		expect(harness.remote.batches).toHaveLength(0);
	});
});

describe('reconcile', () => {
	test('a push failure leaves the outbox intact and reports a typed error', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		await harness.engine.enqueueFileChange(change('a.md', 'a2'));
		harness.remote.failNextPush = new Error('network');

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.PushFailed,
		});
		expect(harness.persisted().outbox).toHaveLength(1);

		const result = await harness.engine.reconcile();
		expect(result.pushed).toBe(1);
		expect(harness.persisted().outbox).toHaveLength(0);
	});

	test('flushes the outbox in order in batches of at most 100', async () => {
		const harness = createHarness();
		const enqueued: string[] = [];
		for (let index = 0; index < 250; index += 1) {
			const path = `file-${String(index).padStart(3, '0')}.md`;
			await harness.storage.write({
				path,
				bytes: bytes(path),
				expectedRevision: null,
			});
			enqueued.push(
				(await harness.engine.enqueueFileChange(change(path, `c${index}`)))
					.operationId,
			);
		}

		const result = await harness.engine.reconcile();

		expect(harness.remote.batches.map((batch) => batch.length)).toEqual([
			100, 100, 50,
		]);
		expect(harness.remote.batches.flat().map((op) => op.operationId)).toEqual(
			enqueued,
		);
		expect(result.pushed).toBe(250);
		expect(harness.persisted().outbox).toHaveLength(0);
	});

	test('applies add, update, delete, and move', async () => {
		const harness = createHarness();
		const update = await harness.storage.write({
			path: 'update.md',
			bytes: bytes('before'),
			expectedRevision: null,
		});
		const remove = await harness.storage.write({
			path: 'delete.md',
			bytes: bytes('gone'),
			expectedRevision: null,
		});
		await harness.storage.write({
			path: 'source.md',
			bytes: bytes('moving'),
			expectedRevision: null,
		});
		harness.remote.pages = [
			page(
				[
					change('added.md', 'added'),
					change('update.md', 'after', { baseRevision: update.revision }),
					change('delete.md', null, { baseRevision: remove.revision }),
					change('destination.md', 'moving', { previousPath: 'source.md' }),
				],
				'42',
			),
		];

		const result = await harness.engine.reconcile();

		expect(result.applied).toBe(4);
		expect(result.conflicts).toEqual([]);
		expect(result.cursor).toBe('42');
		expect(
			decoder.decode((await harness.storage.read('added.md'))!.bytes),
		).toBe('added');
		expect(
			decoder.decode((await harness.storage.read('update.md'))!.bytes),
		).toBe('after');
		expect(await harness.storage.read('delete.md')).toBeNull();
		expect(await harness.storage.read('source.md')).toBeNull();
		expect(
			decoder.decode((await harness.storage.read('destination.md'))!.bytes),
		).toBe('moving');

		const persisted = harness.persisted();
		expect(persisted.cursor).toBe('42');
		expect(persisted.knownPaths).toContain('destination.md');
		expect(persisted.knownPaths).not.toContain('source.md');
		expect(persisted.knownPaths).not.toContain('delete.md');
	});

	test('records a revision mismatch without clobbering local bytes', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'file.md',
			bytes: bytes('local'),
			expectedRevision: null,
		});
		harness.remote.pages = [
			page([change('file.md', 'remote', { baseRevision: 'deadbeef' })], '7'),
		];

		const result = await harness.engine.reconcile();

		expect(result.applied).toBe(0);
		expect(result.conflicts).toHaveLength(1);
		expect(result.conflicts[0]).toMatchObject({
			path: 'file.md',
			reason: 'revision_mismatch',
		});
		expect(decoder.decode((await harness.storage.read('file.md'))!.bytes)).toBe(
			'local',
		);
		expect(harness.persisted().cursor).toBe('7');
		expect(harness.persisted().conflicts).toHaveLength(1);
	});

	test('records an occupied destination without moving or overwriting', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'destination.md',
			bytes: bytes('existing'),
			expectedRevision: null,
		});
		await harness.storage.write({
			path: 'source.md',
			bytes: bytes('moving'),
			expectedRevision: null,
		});
		harness.remote.pages = [
			page(
				[change('destination.md', 'moving', { previousPath: 'source.md' })],
				'3',
			),
		];

		const result = await harness.engine.reconcile();

		expect(result.applied).toBe(0);
		expect(result.conflicts[0]).toMatchObject({
			path: 'destination.md',
			reason: 'occupied_destination',
		});
		expect(
			decoder.decode((await harness.storage.read('destination.md'))!.bytes),
		).toBe('existing');
		expect(
			decoder.decode((await harness.storage.read('source.md'))!.bytes),
		).toBe('moving');
	});

	test('records a remote add onto a present path as a conflict', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'existing.md',
			bytes: bytes('local'),
			expectedRevision: null,
		});
		harness.remote.pages = [page([change('existing.md', 'remote')], '4')];

		const result = await harness.engine.reconcile();

		expect(result.conflicts[0]).toMatchObject({
			path: 'existing.md',
			reason: 'unexpected_file',
		});
		expect(
			decoder.decode((await harness.storage.read('existing.md'))!.bytes),
		).toBe('local');
	});

	test('advances the cursor only after a page is fully applied or recorded', async () => {
		const failingCodec: FileChangeCodec = {
			async sealFileChange(input) {
				return encodeFileChange(input);
			},
			async openFileChange(operation) {
				const payload = JSON.parse(
					decoder.decode(fromBase64(operation.ciphertext)),
				) as { path: string };
				if (payload.path === 'second.md') throw new Error('malformed');
				return openCanonical(operation);
			},
		};
		const harness = createHarness({ codec: failingCodec });
		harness.remote.pages = [
			page([change('first.md', 'one'), change('second.md', 'two')], '9'),
		];

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidOperation,
		});

		expect(harness.persisted().cursor).toBe('0');
		expect(
			decoder.decode((await harness.storage.read('first.md'))!.bytes),
		).toBe('one');
		expect(harness.persisted().pushedRevisions['first.md']).toBeDefined();
	});

	test('rejects a malformed cursor before calling the remote', async () => {
		const state = createEmptySyncState();
		state.cursor = '007';
		const harness = createHarness({ state });

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidCursor,
		});
		expect(harness.remote.batches).toHaveLength(0);
	});

	test('rejects a malformed operation without advancing the cursor', async () => {
		const harness = createHarness();
		const malformed = {
			...encodeFileChange(change('x.md', 'x')),
			ciphertext: 'not base64!!',
		};
		harness.remote.pages = [
			{
				accessRevision: '0',
				cursor: '1',
				hasMore: false,
				operations: [{ ...malformed, sequence: '1' }],
			},
		];

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidOperation,
		});
		expect(harness.persisted().cursor).toBe('0');
	});

	test('rejects a push response that does not match the batch', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		await harness.engine.enqueueFileChange(change('a.md', 'a2'));
		harness.remote.push = async () => ({ sequences: [] });

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidResponse,
		});
		expect(harness.persisted().outbox).toHaveLength(1);
	});
});

describe('snapshotLocalChanges', () => {
	test('detects added, changed, and deleted paths and ignores unchanged', async () => {
		const storage = new MemorySyncStorage();
		const keep = await storage.write({
			path: 'keep.md',
			bytes: bytes('keep'),
			expectedRevision: null,
		});
		const edit = await storage.write({
			path: 'edit.md',
			bytes: bytes('v1'),
			expectedRevision: null,
		});
		const gone = await storage.write({
			path: 'gone.md',
			bytes: bytes('gone'),
			expectedRevision: null,
		});
		const state: SyncState = {
			cursor: '5',
			pushedRevisions: {
				'keep.md': keep.revision,
				'edit.md': edit.revision,
				'gone.md': gone.revision,
			},
			knownPaths: ['keep.md', 'edit.md', 'gone.md'],
			outbox: [],
			conflicts: [],
		};
		const harness = createHarness({ storage, state });

		await storage.write({
			path: 'edit.md',
			bytes: bytes('v2'),
			expectedRevision: edit.revision,
		});
		await storage.write({
			path: 'added.md',
			bytes: bytes('new'),
			expectedRevision: null,
		});
		await storage.delete({ path: 'gone.md', expectedRevision: gone.revision });

		const changes = await harness.engine.snapshotLocalChanges();

		expect(changes.map((entry) => entry.path)).toEqual([
			'added.md',
			'edit.md',
			'gone.md',
		]);
		const byPath = new Map(changes.map((entry) => [entry.path, entry]));
		expect(byPath.get('added.md')!.baseRevision).toBeNull();
		expect(decoder.decode(byPath.get('added.md')!.content!)).toBe('new');
		expect(byPath.get('edit.md')!.baseRevision).toBe(edit.revision);
		expect(byPath.get('gone.md')!.content).toBeNull();
		expect(byPath.get('gone.md')!.baseRevision).toBe(gone.revision);
	});
});

describe('revocation lock state', () => {
	test('a revoked remote locks the engine and refuses further work', async () => {
		const harness = createHarness();
		await harness.storage.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		await harness.engine.enqueueFileChange(change('a.md', 'a2'));
		harness.remote.revokedOnPush = true;

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.Revoked,
		});
		expect(harness.engine.locked).toBe(true);
		expect(harness.revoked).toHaveLength(1);

		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.Locked,
		});
		await expect(
			harness.engine.enqueueFileChange(change('a.md', 'a3')),
		).rejects.toMatchObject({ code: BrowserSyncEngineErrorCode.Locked });
	});

	test('a revoked codec locks the engine', async () => {
		const revokedCodec: FileChangeCodec = {
			async sealFileChange() {
				throw revokedError();
			},
			async openFileChange() {
				throw revokedError();
			},
		};
		const harness = createHarness({ codec: revokedCodec });

		await expect(
			harness.engine.enqueueFileChange(change('a.md', 'a')),
		).rejects.toMatchObject({ code: BrowserSyncEngineErrorCode.Revoked });
		expect(harness.engine.locked).toBe(true);
		expect(harness.revoked).toHaveLength(1);
	});

	test('lock() clears keys through onRevoked and refuses reconcile', async () => {
		const harness = createHarness();

		await harness.engine.lock('manual');

		expect(harness.engine.locked).toBe(true);
		expect(harness.revoked).toEqual(['manual']);
		await expect(harness.engine.reconcile()).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.Locked,
		});
	});
});

describe('sync-v1 conformance', () => {
	test('a fixture file change round-trips through the codec boundary', async () => {
		const valid = (
			fixture as {
				valid: Array<{
					input: Record<string, unknown>;
					canonical: string;
				}>;
			}
		).valid;
		const vector = valid[0]!;
		const input = vector.input;
		const path = input.path as string;
		const content = input.content as string;
		const descriptor: FileChange = {
			path,
			previousPath: (input.previousPath as string | null) ?? null,
			baseRevision: (input.baseRevision as string | null) ?? null,
			content: fromBase64(content),
		};

		const codec = createFakeCodec();
		const sealed = await codec.sealFileChange(descriptor);
		const opened = await codec.openFileChange(sealed);

		expect(canonicalFileChange(opened)).toBe(vector.canonical);
		expect(opened.path).toBe(path);
		expect(toBase64(opened.content!)).toBe(content);
	});
});

describe('browser storage adapter', () => {
	test('bridges a BrowserWorkspaceStorage-like replica', async () => {
		const files = new Map<string, Uint8Array>();
		const storage: BrowserWorkspaceStorageLike = {
			async read(path) {
				const existing = files.get(path);
				return existing === undefined
					? null
					: { bytes: existing.slice(), revision: memoryRevision(existing) };
			},
			async write({ path, bytes: input, expectedRevision }) {
				const current = files.get(path);
				if (expectedRevision === null && current !== undefined)
					throw new Error('path exists');
				if (
					typeof expectedRevision === 'string' &&
					(current === undefined ||
						memoryRevision(current) !== expectedRevision)
				)
					throw new Error('stale revision');
				files.set(path, input.slice());
				return { revision: memoryRevision(input) };
			},
			async move({ from, to, expectedRevision }) {
				const source = files.get(from);
				if (source === undefined) throw new Error('missing source');
				if (memoryRevision(source) !== expectedRevision)
					throw new Error('stale revision');
				if (files.has(to)) throw new Error('path exists');
				files.set(to, source.slice());
				files.delete(from);
				return { revision: memoryRevision(source) };
			},
			async delete({ path, expectedRevision }) {
				const current = files.get(path);
				if (current === undefined) throw new Error('missing');
				if (memoryRevision(current) !== expectedRevision)
					throw new Error('stale revision');
				files.delete(path);
			},
		};
		const adapted = createBrowserStorageAdapter({
			storage,
			list: async () => [...files.keys()].sort(),
		});

		const written = await adapted.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		expect((await adapted.read('a.md'))!.revision).toBe(written.revision);

		await adapted.move({
			from: 'a.md',
			to: 'b.md',
			expectedRevision: written.revision,
		});
		expect(await adapted.read('a.md')).toBeNull();
		expect(await adapted.list()).toEqual(['b.md']);

		await adapted.delete({
			path: 'b.md',
			expectedRevision: written.revision,
		});
		expect(await adapted.list()).toEqual([]);
	});
});
