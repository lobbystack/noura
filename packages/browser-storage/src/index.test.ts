import { describe, expect, test } from 'bun:test';
import type { WorkspaceFormat } from '@noura/workspace-format-wasm';
import {
	BrowserStorageError,
	BrowserWorkspaceStorage,
	DuplicateObjectIdentityError,
	type BrowserStorageFileSystem,
	type BrowserStorageLock,
} from './index';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

class FakeFileSystem implements BrowserStorageFileSystem {
	readonly files = new Map<string, Uint8Array>();
	#interruptAfterWrite: string | null = null;
	#interruptAfterRemove: string | null = null;

	interruptOnceAfterWriting(path: string) {
		this.#interruptAfterWrite = path;
	}

	interruptOnceAfterRemoving(path: string) {
		this.#interruptAfterRemove = path;
	}

	async read(path: string, maxBytes?: number) {
		const value = this.files.get(path);
		if (value && maxBytes !== undefined && value.byteLength > maxBytes)
			throw new BrowserStorageError(
				'snapshot_too_large',
				'file would exceed the requested read limit',
			);
		return value?.slice() ?? null;
	}

	async write(path: string, bytes: Uint8Array) {
		this.files.set(path, bytes.slice());
		if (this.#interruptAfterWrite === path) {
			this.#interruptAfterWrite = null;
			throw new Error('interrupted after bytes reached storage');
		}
	}

	async remove(path: string) {
		const removed = this.files.delete(path);
		if (this.#interruptAfterRemove === path) {
			this.#interruptAfterRemove = null;
			throw new Error('interrupted after removal reached storage');
		}
		return removed;
	}

	async list(path: string) {
		const prefix = path.length === 0 ? '' : `${path}/`;
		const entries = new Map<string, 'file' | 'directory'>();
		for (const key of this.files.keys()) {
			if (!key.startsWith(prefix)) continue;
			const remainder = key.slice(prefix.length);
			const separator = remainder.indexOf('/');
			if (separator === -1) entries.set(remainder, 'file');
			else entries.set(remainder.slice(0, separator), 'directory');
		}
		return [...entries].map(([name, kind]) => ({ name, kind }));
	}
}

const lock: BrowserStorageLock = { run: (operation) => operation() };
const format = {
	contentRevision(bytes) {
		return `revision:${decoder.decode(bytes)}`;
	},
	parseMarkdown(relativePath, bytes) {
		const value = decoder.decode(bytes);
		if (value.startsWith('malformed'))
			return {
				kind: 'malformed' as const,
				title: '',
				body: '',
				error: 'bad yaml',
			};
		const id = value.match(/^id:(\S+)/u)?.[1];
		return id
			? {
					kind: 'managed' as const,
					id,
					type: 'note',
					title: id,
					body: '',
					relativePath,
					revision: `revision:${value}`,
					created: null,
					updated: null,
					properties: {},
				}
			: { kind: 'unmanaged' as const, title: '', body: '', frontmatter: null };
	},
} as WorkspaceFormat;

function createStorage(fileSystem = new FakeFileSystem()) {
	return {
		fileSystem,
		storage: new BrowserWorkspaceStorage({ fileSystem, format, lock }),
	};
}

describe('BrowserWorkspaceStorage', () => {
	test('writes canonical bytes only after matching the expected revision', async () => {
		const { storage } = createStorage();
		const created = await storage.write({
			path: 'notes/one.md',
			bytes: encoder.encode('first'),
			expectedRevision: null,
		});

		expect(created.revision).toBe('revision:first');
		await expect(
			storage.write({
				path: 'notes/one.md',
				bytes: encoder.encode('second'),
				expectedRevision: 'revision:stale',
			}),
		).rejects.toMatchObject({ code: 'stale_revision' });
		expect(decoder.decode((await storage.read('notes/one.md'))?.bytes)).toBe(
			'first',
		);
	});

	test('recovers a mutation interrupted after its target write on reload', async () => {
		const fileSystem = new FakeFileSystem();
		const first = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		const initial = await first.write({
			path: 'notes/one.md',
			bytes: encoder.encode('before'),
			expectedRevision: null,
		});
		fileSystem.interruptOnceAfterWriting('notes/one.md');

		await expect(
			first.write({
				path: 'notes/one.md',
				bytes: encoder.encode('after'),
				expectedRevision: initial.revision,
			}),
		).rejects.toThrow('interrupted');

		const reloaded = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		await reloaded.recover();
		expect(decoder.decode((await reloaded.read('notes/one.md'))?.bytes)).toBe(
			'after',
		);
		expect(await fileSystem.list('.noura/browser-storage/journals')).toEqual(
			[],
		);
	});

	test('moves and deletes only when both expected files remain current', async () => {
		const { storage } = createStorage();
		const created = await storage.write({
			path: 'notes/one.md',
			bytes: encoder.encode('contents'),
			expectedRevision: null,
		});
		const moved = await storage.move({
			from: 'notes/one.md',
			to: 'archive/one.md',
			expectedRevision: created.revision,
			expectedDestinationRevision: null,
		});

		expect(await storage.read('notes/one.md')).toBeNull();
		expect(decoder.decode((await storage.read('archive/one.md'))?.bytes)).toBe(
			'contents',
		);
		await storage.delete({
			path: 'archive/one.md',
			expectedRevision: moved.revision,
		});
		expect(await storage.read('archive/one.md')).toBeNull();
	});

	test('keeps a move journal until source deletion completes for recovery', async () => {
		const fileSystem = new FakeFileSystem();
		const first = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		const created = await first.write({
			path: 'notes/one.md',
			bytes: encoder.encode('contents'),
			expectedRevision: null,
		});
		fileSystem.interruptOnceAfterRemoving('notes/one.md');

		await expect(
			first.move({
				from: 'notes/one.md',
				to: 'archive/one.md',
				expectedRevision: created.revision,
				expectedDestinationRevision: null,
			}),
		).rejects.toThrow('interrupted');
		expect(
			await fileSystem.list('.noura/browser-storage/journals'),
		).not.toEqual([]);

		const reloaded = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		await reloaded.recover();
		expect(await reloaded.read('notes/one.md')).toBeNull();
		expect(decoder.decode((await reloaded.read('archive/one.md'))?.bytes)).toBe(
			'contents',
		);
		expect(await fileSystem.list('.noura/browser-storage/journals')).toEqual(
			[],
		);
	});

	test('rejects malformed, traversing, reserved, and host-specific paths', async () => {
		const { storage } = createStorage();
		for (const path of [
			'',
			'/absolute.md',
			'../escape.md',
			'notes//two.md',
			'notes\\two.md',
			'notes/CON.md',
			'notes/\ud800.md',
			'.noura/browser-storage/journals/forged.json',
		]) {
			await expect(
				storage.write({
					path,
					bytes: encoder.encode('x'),
					expectedRevision: null,
				}),
			).rejects.toBeInstanceOf(BrowserStorageError);
		}
	});

	test('rebuild enumerates canonical files without an index and excludes recovery data', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_first'));
		await fileSystem.write(
			'.noura/browser-storage/staged/orphan.bin',
			encoder.encode('orphan'),
		);

		const rebuilt = await storage.rebuild();
		expect(rebuilt.files).toEqual([
			{ path: 'notes/a.md', revision: 'revision:id:note_first' },
		]);
		expect(rebuilt.managed.map((file) => file.id)).toEqual(['note_first']);
	});

	test('rebuilds from files and rejects duplicate stable identities', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_same'));
		await fileSystem.write('notes/b.md', encoder.encode('id:note_same'));
		await fileSystem.write('notes/bad.md', encoder.encode('malformed'));

		await expect(storage.rebuild()).rejects.toBeInstanceOf(
			DuplicateObjectIdentityError,
		);
	});

	test('retains durable metadata without treating history as a duplicate live object', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_same'));
		await fileSystem.write(
			'.noura/history/note_same/previous.md',
			encoder.encode('id:note_same'),
		);

		await expect(storage.rebuild()).resolves.toMatchObject({
			files: [
				{
					path: '.noura/history/note_same/previous.md',
					revision: 'revision:id:note_same',
				},
				{ path: 'notes/a.md', revision: 'revision:id:note_same' },
			],
			managed: [expect.objectContaining({ id: 'note_same' })],
		});
	});

	test('enforces export limits before copying a file into the snapshot', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.bin', encoder.encode('too large'));

		await expect(
			storage.exportSnapshotEntries({
				maxEntries: 1,
				maxEntryBytes: 8,
				maxTotalBytes: 8,
			}),
		).rejects.toMatchObject({ code: 'snapshot_too_large' });
	});

	test('exports every durable file byte-for-byte except adapter and derived metadata', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/raw.bin', new Uint8Array([0, 255, 1]));
		await fileSystem.write('.noura/workspace.yaml', encoder.encode('manifest'));
		await fileSystem.write('.noura/history/note/old.md', encoder.encode('old'));
		await fileSystem.write('.noura/index.sqlite', encoder.encode('derived'));
		await fileSystem.write(
			'.noura/browser-storage/staged/transient.bin',
			encoder.encode('transient'),
		);

		const exported = await storage.exportSnapshotEntries();
		expect(exported.map((entry) => entry.path)).toEqual([
			'.noura/history/note/old.md',
			'.noura/workspace.yaml',
			'notes/raw.bin',
		]);
		expect(
			exported.find((entry) => entry.path === 'notes/raw.bin')?.bytes,
		).toEqual(new Uint8Array([0, 255, 1]));
	});

	test('recovers an interrupted fresh snapshot import without overwriting existing files', async () => {
		const fileSystem = new FakeFileSystem();
		const first = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		fileSystem.interruptOnceAfterWriting('.noura/workspace.yaml');
		await expect(
			first.importSnapshotEntries([
				{ path: '.noura/workspace.yaml', bytes: encoder.encode('manifest') },
				{ path: 'notes/one.md', bytes: encoder.encode('id:note_one') },
			]),
		).rejects.toThrow('interrupted');

		const reloaded = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		await reloaded.recover();
		expect(decoder.decode((await reloaded.read('notes/one.md'))?.bytes)).toBe(
			'id:note_one',
		);
		await expect(
			reloaded.importSnapshotEntries([
				{ path: '.noura/workspace.yaml', bytes: encoder.encode('other') },
			]),
		).rejects.toMatchObject({ code: 'workspace_not_empty' });
	});

	test('rejects duplicate and traversing import entries before any canonical write', async () => {
		const { fileSystem, storage } = createStorage();
		await expect(
			storage.importSnapshotEntries([
				{ path: 'notes/a.md', bytes: encoder.encode('a') },
				{ path: 'notes/a.md', bytes: encoder.encode('b') },
			]),
		).rejects.toMatchObject({ code: 'invalid_snapshot' });
		await expect(
			storage.importSnapshotEntries([
				{ path: '../outside.md', bytes: encoder.encode('a') },
			]),
		).rejects.toBeInstanceOf(BrowserStorageError);
		expect(fileSystem.files.size).toBe(0);
	});

	test('refuses a malformed move recovery journal before it can delete its target', async () => {
		const { fileSystem, storage } = createStorage();
		const id = '00000000-0000-0000-0000-000000000001';
		await fileSystem.write('notes/a.md', encoder.encode('id:note_one'));
		await fileSystem.write(
			`.noura/browser-storage/journals/${id}.json`,
			encoder.encode(
				JSON.stringify({
					version: 1,
					id,
					kind: 'move',
					from: 'notes/a.md',
					to: 'notes/a.md',
				}),
			),
		);

		await expect(storage.recover()).rejects.toMatchObject({
			code: 'recovery_failed',
		});
		expect(decoder.decode((await fileSystem.read('notes/a.md'))!)).toBe(
			'id:note_one',
		);
	});
});
