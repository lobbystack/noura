// Seam 5: the package command writes the release archive, and Noura
// installs and runs the plugin from it.
import { readFile, rename, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { ARCHIVE_SUFFIX } from '@noura/plugin-cli';
import { unzipSync } from 'fflate';
import { expect, test } from 'vitest';

import { inTemporaryFolder, succeed } from './support/kit.js';
import { RealHost, texts } from './support/real-host.js';

test('one command writes an archive of the manifest and the component', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');

		const output = await succeed('noura-plugin', ['package'], project);

		expect(output).toContain(
			'Wrote the release archive dist/word-count-0.1.0.noura-plugin.zip.',
		);
		const archive = unzipSync(
			await readFile(join(project, 'dist/word-count-0.1.0.noura-plugin.zip')),
		);
		expect(Object.keys(archive).sort()).toEqual([
			'dist/plugin.wasm',
			'plugin.toml',
		]);
		expect(archive['plugin.toml']).toEqual(
			new Uint8Array(await readFile(join(project, 'plugin.toml'))),
		);
		expect(archive['dist/plugin.wasm']).toEqual(
			new Uint8Array(await readFile(join(project, 'dist/plugin.wasm'))),
		);
	});
});

test('the archive’s name ends as Noura looks for on a GitHub release', async (context) => {
	const host = await RealHost.start();
	if (!host) return context.skip();
	try {
		expect(await host.archiveSuffix()).toBe(ARCHIVE_SUFFIX);
	} finally {
		await host.close();
	}
});

test('Noura installs the archive and runs the plugin from it', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		await succeed('noura-plugin', ['package'], project);
		const archive = join(folder, 'word-count-0.1.0.noura-plugin.zip');
		await rename(
			join(project, 'dist/word-count-0.1.0.noura-plugin.zip'),
			archive,
		);
		// Nothing of the project is left: the archive holds the plugin.
		await rm(project, { recursive: true });

		const host = await RealHost.start({ 'Plan.md': 'plan' });
		if (!host) {
			return;
		}
		try {
			expect(await host.installArchive(archive)).toMatchObject({
				id: 'word-count',
				running: true,
				problem: null,
				approved: ['workspace-read', 'views'],
			});
			const answer = await host.view('plugin:word-count:main');
			expect(answer && 'view' in answer ? texts(answer.view) : answer).toEqual([
				'Your workspace holds 1 file.',
				'You pressed the button 0 times.',
			]);
		} finally {
			await host.close();
		}
	});
});
