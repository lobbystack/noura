// Seam 5: dev mode builds the plugin again on each save, and Noura, which
// installed the folder, reloads it.
import { readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { inTemporaryFolder, start, succeed } from './support/kit.js';
import { RealHost, texts } from './support/real-host.js';

/** Poll the page until it shows `text`, for up to a minute. */
async function waitForText(
	host: RealHost,
	page: string,
	text: string,
): Promise<string[]> {
	const deadline = Date.now() + 60_000;
	let shown: string[] = [];
	while (Date.now() < deadline) {
		const answer = await host.view(page);
		shown = answer && 'view' in answer ? texts(answer.view) : [];
		if (shown.includes(text)) {
			return shown;
		}
		await new Promise((wait) => setTimeout(wait, 100));
	}
	throw new Error(
		`The page never showed “${text}”. It shows ${JSON.stringify(shown)}.`,
	);
}

test('saving the source while dev mode runs shows the new build in Noura', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		const dev = start('noura-plugin', ['dev'], project);
		try {
			await dev.waitFor(/^Waiting for changes/);
			expect(dev.output()).toMatch(
				/^Built Word count 0\.1\.0 into dist\/plugin\.wasm in [0-9.]+ s\.$/m,
			);

			const host = await RealHost.start({ 'Plan.md': 'plan' });
			if (!host) {
				return;
			}
			try {
				expect(await host.install(project)).toMatchObject({
					running: true,
					problem: null,
				});
				const page = 'plugin:word-count:main';
				await waitForText(host, page, 'Your workspace holds 1 file.');

				const source = join(project, 'src/plugin.ts');
				const text = await readFile(source, 'utf8');
				await writeFile(
					source,
					text.replace('Your workspace holds', 'This workspace holds'),
				);

				await dev.waitFor(/^Built Word count/, 2);
				expect(
					await waitForText(host, page, 'This workspace holds 1 file.'),
				).toEqual([
					'This workspace holds 1 file.',
					'You pressed the button 0 times.',
				]);
				expect(await host.plugin('word-count')).toMatchObject({
					running: true,
					problem: null,
				});
			} finally {
				await host.close();
			}
		} finally {
			await dev.stop();
		}
	});
});

test('a save that breaks the build says why and keeps watching', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		const dev = start('noura-plugin', ['dev'], project);
		try {
			await dev.waitFor(/^Waiting for changes/);
			const source = join(project, 'src/plugin.ts');
			const text = await readFile(source, 'utf8');

			await writeFile(source, `${text}\nexport const broken = ;\n`);
			await dev.waitFor(/^The build failed\./);
			await writeFile(source, text);

			await dev.waitFor(/^Built Word count/, 2);
		} finally {
			await dev.stop();
		}
	});
});

test('a build that can’t write the component says why and keeps watching', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		// A file where the component's folder goes.
		await writeFile(join(project, 'dist'), 'not a folder');
		const dev = start('noura-plugin', ['dev'], project);
		try {
			await dev.waitFor(/^Waiting for changes/);
			expect(dev.output()).toMatch(/^The build failed: /m);

			await rm(join(project, 'dist'));
			const source = join(project, 'src/plugin.ts');
			await writeFile(source, await readFile(source, 'utf8'));

			await dev.waitFor(/^Built Word count/);
		} finally {
			await dev.stop();
		}
	});
});

test('an entry inside src is refused, so a build never starts the next one', async () => {
	await inTemporaryFolder(async (folder) => {
		await succeed('create-noura-plugin', ['word-count'], folder);
		const project = join(folder, 'word-count');
		const manifest = join(project, 'plugin.toml');
		const text = await readFile(manifest, 'utf8');
		await writeFile(
			manifest,
			text.replace('entry = "dist/plugin.wasm"', 'entry = "src/plugin.wasm"'),
		);
		const dev = start('noura-plugin', ['dev'], project);
		try {
			await dev.waitFor(/^Waiting for changes/);
			expect(dev.output()).toMatch(
				/^The entry src\/plugin\.wasm in plugin\.toml is inside src/m,
			);

			await writeFile(manifest, text);

			await dev.waitFor(/^Built Word count/);
			await new Promise((wait) => setTimeout(wait, 1_000));
			expect(
				dev
					.output()
					.split('\n')
					.filter((line) => /^(Built|The entry)/.test(line)),
			).toHaveLength(2);
		} finally {
			await dev.stop();
		}
	});
});
