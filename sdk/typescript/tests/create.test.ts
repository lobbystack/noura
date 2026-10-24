// Seam 5: create-noura-plugin writes a project whose example builds and
// runs in the real host on the first try.
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { inTemporaryFolder, run, succeed } from './support/kit.js';
import { RealHost, type ViewAnswer, texts } from './support/real-host.js';

function shown(answer: ViewAnswer): string[] {
	if (!answer || !('view' in answer)) {
		throw new Error(`The page shows no view: ${JSON.stringify(answer)}`);
	}
	return texts(answer.view);
}

test('the project holds a manifest, the typed interfaces, an example and its test', async () => {
	await inTemporaryFolder(async (folder) => {
		const output = await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');

		expect(output).toContain('word-count');
		expect((await readdir(project)).sort()).toEqual([
			'.gitignore',
			'README.md',
			'package.json',
			'plugin.toml',
			'src',
			'test',
			'tsconfig.json',
		]);
		const manifest = await readFile(join(project, 'plugin.toml'), 'utf8');
		expect(manifest).toContain('id = "word-count"');
		expect(manifest).toContain('name = "Word count"');
		expect(manifest).toContain('interface-version = "0.1.0"');
		const source = await readFile(join(project, 'src/plugin.ts'), 'utf8');
		expect(source).toContain("from 'noura:plugin/workspace@0.1.0'");
		const pkg = JSON.parse(await readFile(join(project, 'package.json'), 'utf8'));
		expect(pkg.name).toBe('word-count');
		expect(pkg.devDependencies).toMatchObject({
			'@noura/plugin-cli': '^0.1.0',
			'@noura/plugin-types': '^0.1.0',
			'@noura/mock-host': '^0.1.0',
		});
		const tsconfig = JSON.parse(await readFile(join(project, 'tsconfig.json'), 'utf8'));
		expect(tsconfig.compilerOptions.types).toContain('@noura/plugin-types');
	});
});

test('the example builds and runs in Noura on the first try', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		await succeed('noura-plugin', ['build'], project);

		const host = await RealHost.start({ 'Plan.md': 'plan', 'Notes/Idea.md': 'idea' });
		if (!host) {
			return;
		}
		try {
			expect(await host.install(project)).toMatchObject({
				id: 'word-count',
				name: 'Word count',
				running: true,
				problem: null,
			});
			const page = 'plugin:word-count:main';
			expect(await host.pages()).toEqual([{ id: page, title: 'Word count' }]);
			expect(shown(await host.view(page))).toEqual([
				'Your workspace holds 2 files.',
				'You pressed the button 0 times.',
			]);

			await host.send(page, { tag: 'pressed', val: 'press' });

			expect(shown(await host.view(page))).toEqual([
				'Your workspace holds 2 files.',
				'You pressed the button 1 time.',
			]);
		} finally {
			await host.close();
		}
	});
});

test('a folder that is taken or a name Noura can’t use writes nothing', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['taken'], folder);

		const again = await run('create-noura-plugin', ['taken'], folder);
		expect(again.code).toBe(1);
		expect(again.output).toContain('taken');

		const invalid = await run('create-noura-plugin', ['9 Lives'], folder);
		expect(invalid.code).toBe(1);
		expect((await readdir(folder)).sort()).toEqual(['taken']);
	});
});
