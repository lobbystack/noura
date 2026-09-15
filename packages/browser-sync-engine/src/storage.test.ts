import { describe, expect, test } from 'bun:test';
import {
	BrowserSyncEngineErrorCode,
	createWorkspaceStorageAdapter,
	memoryRevision,
	type WorkspaceRebuildResult,
	type WorkspaceStorageLike,
} from './index';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function bytes(value: string): Uint8Array {
	return encoder.encode(value);
}

class StaleRevisionError extends Error {
	constructor() {
		super('stale revision');
		this.name = 'StaleRevisionError';
	}
}

/** In-memory replica with the same mutation contracts as BrowserWorkspaceStorage. */
class FakeWorkspaceStorage implements WorkspaceStorageLike {
	readonly files = new Map<string, Uint8Array>();
	rebuildCount = 0;
	rebuildResult: WorkspaceRebuildResult | null = null;
	lastWriteExpectedRevision: string | null | undefined;

	async read(path: string) {
		const value = this.files.get(path);
		return value === undefined
			? null
			: { bytes: value.slice(), revision: memoryRevision(value) };
	}

	async write({
		path,
		bytes: input,
		expectedRevision,
	}: {
		path: string;
		bytes: Uint8Array;
		expectedRevision?: string | null;
	}) {
		this.lastWriteExpectedRevision = expectedRevision;
		const current = this.files.get(path);
		if (current === undefined) {
			if (expectedRevision !== null && expectedRevision !== undefined) {
				throw new StaleRevisionError();
			}
		} else {
			if (expectedRevision === null) throw new Error('path exists');
			if (
				typeof expectedRevision === 'string' &&
				memoryRevision(current) !== expectedRevision
			) {
				throw new StaleRevisionError();
			}
		}
		this.files.set(path, input.slice());
		return { revision: memoryRevision(input) };
	}

	async move({
		from,
		to,
		expectedRevision,
	}: {
		from: string;
		to: string;
		expectedRevision?: string | null;
	}) {
		const source = this.files.get(from);
		if (source === undefined) throw new Error('missing source');
		if (
			typeof expectedRevision !== 'string' ||
			memoryRevision(source) !== expectedRevision
		) {
			throw new StaleRevisionError();
		}
		if (this.files.has(to)) throw new Error('path exists');
		this.files.set(to, source.slice());
		this.files.delete(from);
		return { revision: memoryRevision(source) };
	}

	async delete({
		path,
		expectedRevision,
	}: {
		path: string;
		expectedRevision?: string | null;
	}) {
		const current = this.files.get(path);
		if (current === undefined) throw new Error('missing');
		if (
			typeof expectedRevision !== 'string' ||
			memoryRevision(current) !== expectedRevision
		) {
			throw new StaleRevisionError();
		}
		this.files.delete(path);
	}

	async rebuild(): Promise<WorkspaceRebuildResult> {
		this.rebuildCount += 1;
		if (this.rebuildResult !== null) return this.rebuildResult;
		return {
			files: [...this.files.keys()].sort().map((path) => ({
				path,
				revision: memoryRevision(this.files.get(path)!),
			})),
			managed: [],
		};
	}
}

describe('createWorkspaceStorageAdapter', () => {
	test('derives list from rebuild for ordinary and managed files', async () => {
		const workspace = new FakeWorkspaceStorage();
		workspace.rebuildResult = {
			files: [
				{ path: 'notes/ordinary.md' },
				{ path: 'notes/managed.md' },
				{ path: '.noura/workspace.yaml' },
			],
			managed: [
				{ relativePath: 'notes/managed.md' },
				{ path: 'notes/legacy.md' },
			],
		};
		const adapter = createWorkspaceStorageAdapter(workspace);

		expect(await adapter.list()).toEqual([
			'.noura/workspace.yaml',
			'notes/legacy.md',
			'notes/managed.md',
			'notes/ordinary.md',
		]);
		expect(workspace.rebuildCount).toBe(1);
	});

	test('reads bytes and revision without exposing the storage path', async () => {
		const workspace = new FakeWorkspaceStorage();
		const stored = await workspace.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		const adapter = createWorkspaceStorageAdapter(workspace);

		const file = await adapter.read('a.md');
		expect(file).not.toBeNull();
		expect(decoder.decode(file!.bytes)).toBe('a');
		expect(file!.revision).toBe(stored.revision);
		expect('path' in file!).toBe(false);

		expect(await adapter.read('missing.md')).toBeNull();
	});

	test('write returns the revision and propagates expectedRevision', async () => {
		const workspace = new FakeWorkspaceStorage();
		const adapter = createWorkspaceStorageAdapter(workspace);

		const created = await adapter.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		expect(created.revision).toBe(memoryRevision(bytes('a')));
		expect(workspace.lastWriteExpectedRevision).toBeNull();

		const updated = await adapter.write({
			path: 'a.md',
			bytes: bytes('b'),
			expectedRevision: created.revision,
		});
		expect(updated.revision).toBe(memoryRevision(bytes('b')));
		expect(workspace.lastWriteExpectedRevision).toBe(created.revision);
	});

	test('write propagates stale and unexpected-file rejections', async () => {
		const workspace = new FakeWorkspaceStorage();
		await workspace.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		const adapter = createWorkspaceStorageAdapter(workspace);

		await expect(
			adapter.write({
				path: 'a.md',
				bytes: bytes('b'),
				expectedRevision: 'stale',
			}),
		).rejects.toBeInstanceOf(StaleRevisionError);

		await expect(
			adapter.write({
				path: 'a.md',
				bytes: bytes('b'),
				expectedRevision: null,
			}),
		).rejects.toThrow('path exists');
	});

	test('moves and deletes using expected revisions', async () => {
		const workspace = new FakeWorkspaceStorage();
		const stored = await workspace.write({
			path: 'a.md',
			bytes: bytes('a'),
			expectedRevision: null,
		});
		const adapter = createWorkspaceStorageAdapter(workspace);

		await adapter.move({
			from: 'a.md',
			to: 'b.md',
			expectedRevision: stored.revision,
		});
		expect(await adapter.read('a.md')).toBeNull();
		expect(decoder.decode((await adapter.read('b.md'))!.bytes)).toBe('a');
		expect(await adapter.list()).toEqual(['b.md']);

		await expect(
			adapter.move({
				from: 'b.md',
				to: 'c.md',
				expectedRevision: 'stale',
			}),
		).rejects.toBeInstanceOf(StaleRevisionError);

		await adapter.delete({
			path: 'b.md',
			expectedRevision: stored.revision,
		});
		expect(await adapter.read('b.md')).toBeNull();
		expect(await adapter.list()).toEqual([]);
	});

	test('rejects a mutation without a current revision', async () => {
		const adapter = createWorkspaceStorageAdapter(new FakeWorkspaceStorage());

		await expect(
			adapter.move({ from: 'a.md', to: 'b.md' }),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidStorageCall,
		});
		await expect(adapter.delete({ path: 'a.md' })).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidStorageCall,
		});
	});
});
