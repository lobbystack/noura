import { describe, expect, test } from 'bun:test';
import type { WorkspaceFormat } from '@noura/workspace-format-wasm';
import {
	BrowserStorageError,
	BrowserWorkspaceStorage,
	type BrowserStorageFileSystem,
	type BrowserStorageLock,
} from './index';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

class FakeFileSystem implements BrowserStorageFileSystem {
	readonly files = new Map<string, Uint8Array>();
	readonly directories = new Set<string>();
	reads = 0;
	lists = 0;
	#interruptAfterWrite: string | null = null;
	#interruptAfterRemove: string | null = null;

	interruptOnceAfterWriting(path: string) {
		this.#interruptAfterWrite = path;
	}

	interruptOnceAfterRemoving(path: string) {
		this.#interruptAfterRemove = path;
	}

	async read(path: string, maxBytes?: number) {
		this.reads += 1;
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

	async makeDirectory(path: string) {
		this.directories.add(path);
	}

	async removeDirectory(path: string, options: { recursive?: boolean } = {}) {
		const prefix = `${path}/`;
		const nested = [...this.files.keys(), ...this.directories].some((key) =>
			key.startsWith(prefix),
		);
		if (nested && !options.recursive) throw new Error('directory not empty');
		for (const key of [...this.files.keys()])
			if (key.startsWith(prefix)) this.files.delete(key);
		for (const key of [...this.directories])
			if (key.startsWith(prefix)) this.directories.delete(key);
		return this.directories.delete(path);
	}

	async list(path: string) {
		this.lists += 1;
		const prefix = path.length === 0 ? '' : `${path}/`;
		const entries = new Map<string, 'file' | 'directory'>();
		for (const key of this.directories) {
			if (!key.startsWith(prefix)) continue;
			entries.set(key.slice(prefix.length).split('/')[0]!, 'directory');
		}
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

	test('reports duplicate stable identities and leaves their files alone', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_same'));
		await fileSystem.write('notes/b.md', encoder.encode('id:note_same'));
		await fileSystem.write('notes/bad.md', encoder.encode('malformed'));
		await fileSystem.write('notes/c.md', encoder.encode('id:note_other'));

		const rebuilt = await storage.rebuild();
		expect(rebuilt.duplicates).toEqual([
			{ id: 'note_same', paths: ['notes/a.md', 'notes/b.md'] },
		]);
		expect(rebuilt.managed.map((file) => file.relativePath)).toEqual([
			'notes/a.md',
			'notes/b.md',
			'notes/c.md',
		]);
		expect(rebuilt.malformedMarkdown).toEqual([
			{ path: 'notes/bad.md', error: 'bad yaml' },
		]);
		expect(decoder.decode(fileSystem.files.get('notes/a.md'))).toBe(
			'id:note_same',
		);
		expect(decoder.decode(fileSystem.files.get('notes/b.md'))).toBe(
			'id:note_same',
		);
	});

	test('reads the files once and keeps the projection in step with writes', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_a'));
		await fileSystem.write('notes/b.md', encoder.encode('id:note_b'));
		await storage.rebuild();
		const readsAfterFirstScan = fileSystem.reads;

		await storage.rebuild();
		await storage.rebuild();
		expect(fileSystem.reads).toBe(readsAfterFirstScan);

		const b = await storage.read('notes/b.md');
		await storage.write({
			path: 'notes/c.md',
			bytes: encoder.encode('id:note_c'),
			expectedRevision: null,
		});
		await storage.move({
			from: 'notes/b.md',
			to: 'archive/b.md',
			expectedRevision: b!.revision,
			expectedDestinationRevision: null,
		});
		await storage.delete({
			path: 'notes/a.md',
			expectedRevision: 'revision:id:note_a',
		});
		const rebuilt = await storage.rebuild();
		expect(rebuilt.managed.map((file) => [file.id, file.relativePath])).toEqual(
			[
				['note_b', 'archive/b.md'],
				['note_c', 'notes/c.md'],
			],
		);
		expect(rebuilt.folders).toEqual(['archive', 'notes']);
	});

	test('sees changes from another context only after invalidation', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_a'));
		await storage.rebuild();

		await fileSystem.write('notes/a.md', encoder.encode('id:note_changed'));
		await fileSystem.write('notes/new.md', encoder.encode('id:note_new'));
		expect((await storage.rebuild()).managed.map((file) => file.id)).toEqual([
			'note_a',
		]);

		await storage.invalidate(['notes/a.md', 'notes/new.md']);
		expect((await storage.rebuild()).managed.map((file) => file.id)).toEqual([
			'note_changed',
			'note_new',
		]);

		fileSystem.files.delete('notes/new.md');
		await storage.invalidate();
		expect((await storage.rebuild()).managed.map((file) => file.id)).toEqual([
			'note_changed',
		]);
	});

	test('only visible workspace files define live objects', async () => {
		const { fileSystem, storage } = createStorage();
		await fileSystem.write('notes/a.md', encoder.encode('id:note_a'));
		await fileSystem.write('.obsidian/a.md', encoder.encode('id:note_a'));
		await fileSystem.write('.noura/trash/t/a.md', encoder.encode('id:note_a'));
		await fileSystem.write('node_modules/x.md', encoder.encode('id:note_x'));
		await fileSystem.write('notes/.hidden.md', encoder.encode('id:note_h'));

		const rebuilt = await storage.rebuild();
		expect(rebuilt.managed.map((file) => file.relativePath)).toEqual([
			'notes/a.md',
		]);
		expect(rebuilt.duplicates).toEqual([]);
		expect(rebuilt.files).toHaveLength(5);
	});

	test('creates, moves and removes folders with everything inside', async () => {
		const { fileSystem, storage } = createStorage();
		await storage.createFolder('Projects/Empty');
		await storage.write({
			path: 'Projects/plan.md',
			bytes: encoder.encode('id:note_plan'),
			expectedRevision: null,
		});
		await expect(storage.createFolder('Projects')).rejects.toMatchObject({
			code: 'path_exists',
		});

		const moves = await storage.movePath({ from: 'Projects', to: 'Archive/P' });
		expect(moves).toEqual([
			{ from: 'Projects/plan.md', to: 'Archive/P/plan.md' },
		]);
		expect(fileSystem.files.has('Projects/plan.md')).toBe(false);
		expect(decoder.decode(fileSystem.files.get('Archive/P/plan.md'))).toBe(
			'id:note_plan',
		);
		const rebuilt = await storage.rebuild();
		expect(rebuilt.folders).toEqual([
			'Archive',
			'Archive/P',
			'Archive/P/Empty',
		]);
		expect(rebuilt.managed[0]?.relativePath).toBe('Archive/P/plan.md');

		await expect(
			storage.movePath({ from: 'Archive', to: 'Archive/P/x' }),
		).rejects.toMatchObject({ code: 'invalid_path' });
		await expect(storage.removeEmptyFolder('Archive/P')).rejects.toMatchObject({
			code: 'folder_not_empty',
		});
		await storage.removeEmptyFolder('Archive/P/Empty');
		expect((await storage.rebuild()).folders).toEqual(['Archive', 'Archive/P']);
	});

	test('finishes an interrupted folder move on the next start', async () => {
		const fileSystem = new FakeFileSystem();
		const first = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		await first.write({
			path: 'A/one.md',
			bytes: encoder.encode('one'),
			expectedRevision: null,
		});
		await first.write({
			path: 'A/two.md',
			bytes: encoder.encode('two'),
			expectedRevision: null,
		});
		fileSystem.interruptOnceAfterRemoving('A/one.md');
		await expect(first.movePath({ from: 'A', to: 'B' })).rejects.toThrow(
			'interrupted',
		);

		const reloaded = new BrowserWorkspaceStorage({ fileSystem, format, lock });
		await reloaded.recover();
		expect(
			[...fileSystem.files.keys()].filter((key) => !key.startsWith('.')).sort(),
		).toEqual(['B/one.md', 'B/two.md']);
		expect(await fileSystem.list('.noura/browser-storage/journals')).toEqual(
			[],
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
