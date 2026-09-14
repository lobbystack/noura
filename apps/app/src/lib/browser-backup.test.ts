import { describe, expect, test } from 'bun:test';
import { JSDOM } from 'jsdom';
import type { BrowserWorkspaceSnapshot } from '@noura/browser-workspace';
import {
	assertBackupActionAllowed,
	assertFreshIdentity,
	BACKUP_LIMITS,
	decodeBackup,
	downloadBackup,
	encodeBackup,
} from './browser-backup';

const snapshot = (): BrowserWorkspaceSnapshot => ({
	format: 'noura.workspace-snapshot',
	version: 1,
	workspaceId: 'workspace-test',
	entries: [
		{
			path: '.noura/workspace.yaml',
			bytes: new TextEncoder().encode('manifest'),
		},
		{
			path: 'assets/日本語.bin',
			bytes: Uint8Array.from({ length: 65537 }, (_, i) => i % 256),
		},
		{ path: 'empty', bytes: new Uint8Array() },
	],
});
async function wire() {
	return JSON.parse(await encodeBackup(snapshot()).text());
}
const blob = (value: unknown) => new Blob([JSON.stringify(value)]);
describe('browser JSON backup', () => {
	test('roundtrips arbitrary binary, empty files, Unicode paths and stable identity', async () => {
		const original = snapshot();
		expect(await decodeBackup(encodeBackup(original))).toEqual(original);
	});
	test('bounds file size before reading', async () => {
		let read = false;
		await expect(
			decodeBackup({
				size: BACKUP_LIMITS.maxFileBytes + 1,
				arrayBuffer() {
					read = true;
					throw new Error();
				},
			} as unknown as Blob),
		).rejects.toThrow('96 MiB');
		expect(read).toBe(false);
	});
	test('rejects hostile shapes, paths, duplicate paths, metadata and encodings', async () => {
		const source = await wire();
		for (const value of [
			null,
			[],
			{},
			{ ...source, version: 2 },
			{ ...source, extra: 1 },
			{ ...source, entries: [] },
			{ ...source, entries: Array(10001).fill(source.entries[0]) },
			...[
				'../escape',
				'/absolute',
				'a\\b',
				'CON.txt',
				'a//b',
				'.noura/browser-storage/staged/x',
				'.noura/index.sqlite',
				'.noura/index.sqlite-shm',
				'.noura/index.sqlite-wal',
				'\ud800',
			].map((path) => ({
				...source,
				entries: [source.entries[0], { path, base64: '' }],
			})),
			{ ...source, entries: [source.entries[0], source.entries[0]] },
			{
				...source,
				entries: [
					source.entries[0],
					{ path: 'a', base64: '' },
					{ path: 'a/b', base64: '' },
				],
			},
			...['Zg=', 'Zh==', 'Zm9=', '====', 'a===', 'AA A', '💥'].map(
				(base64) => ({
					...source,
					entries: [source.entries[0], { path: 'binary', base64 }],
				}),
			),
		])
			await expect(
				decodeBackup(blob(value)),
				JSON.stringify(value),
			).rejects.toThrow();
		await expect(
			decodeBackup(new Blob([new Uint8Array([255])])),
		).rejects.toThrow();
	});
	test('rejects malformed snapshots before base64 encoding', () => {
		for (const value of [
			null,
			{ ...snapshot(), format: 'unexpected' },
			{ ...snapshot(), entries: [{ path: 'notes/a.md', bytes: 'not bytes' }] },
		])
			expect(() =>
				encodeBackup(value as unknown as BrowserWorkspaceSnapshot),
			).toThrow('Invalid or unsupported');
	});
	test('rejects oversized entries before binary decode', async () => {
		const source = await wire();
		await expect(
			decodeBackup(
				blob({
					...source,
					entries: [
						{
							path: '.noura/workspace.yaml',
							base64: 'A'.repeat(
								4 * Math.ceil(BACKUP_LIMITS.maxEntryBytes / 3) + 4,
							),
						},
					],
				}),
			),
		).rejects.toThrow();
	});
	test('guards drafts and identity conflicts without altering IDs', () => {
		expect(() => assertBackupActionAllowed(true)).toThrow('unsaved draft');
		expect(() => assertBackupActionAllowed(false)).not.toThrow();
		const existing = [{ workspaceId: 'same' }];
		expect(() => assertFreshIdentity('same', existing)).toThrow(
			'Nothing was overwritten',
		);
		expect(() => assertFreshIdentity('fresh', existing)).not.toThrow();
		expect(existing).toEqual([{ workspaceId: 'same' }]);
	});
	test('downloads a real Blob with safe filename and delayed cleanup even on failure', () => {
		for (const fail of [false, true]) {
			const dom = new JSDOM();
			let cleanup = () => {};
			let revoked = '';
			const backup = encodeBackup(snapshot());
			const urls = {
				createObjectURL(value: Blob) {
					expect(value).toBe(backup);
					return 'blob:test';
				},
				revokeObjectURL(value: string) {
					revoked = value;
				},
			};
			dom.window.HTMLAnchorElement.prototype.click = function () {
				expect(this.isConnected).toBe(true);
				expect(this.download).toMatch(
					/^noura-[a-zA-Z0-9_-]+\.noura-backup\.json$/,
				);
				if (fail) throw new Error('blocked');
			};
			const action = () =>
				downloadBackup(
					backup,
					'../日本語/<hostile>',
					dom.window.document,
					urls as typeof URL,
					(callback) => {
						cleanup = callback;
						return 0 as unknown as ReturnType<typeof setTimeout>;
					},
				);
			if (fail) expect(action).toThrow('blocked');
			else action();
			expect(dom.window.document.querySelector('a')).toBeNull();
			expect(revoked).toBe('');
			cleanup();
			expect(revoked).toBe('blob:test');
			dom.window.close();
		}
	});
});
