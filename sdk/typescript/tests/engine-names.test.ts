// The kit never names the JavaScript engine it builds with: not in the
// template, the CLI's package configuration, a generated project or the
// CLI's output. Authors depend on the CLI and the generated types only.
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { KIT, inTemporaryFolder, run, start, succeed } from './support/kit.js';

/** The engines jco builds components with, and the tools named after them. */
const ENGINE = /quickjs|qjs|starlingmonkey|spidermonkey/i;

/** Every file under `folder`, by path. */
async function files(folder: string): Promise<string[]> {
	const found: string[] = [];
	for (const entry of await readdir(folder, { withFileTypes: true })) {
		const path = join(folder, entry.name);
		found.push(...(entry.isDirectory() ? await files(path) : [path]));
	}
	return found;
}

async function namingTheEngine(paths: string[]): Promise<string[]> {
	const named: string[] = [];
	for (const path of paths) {
		if (ENGINE.test(path) || ENGINE.test(await readFile(path, 'utf8'))) {
			named.push(path);
		}
	}
	return named;
}

test('the template and the CLI’s package configuration don’t name the engine', async () => {
	const template = await files(join(KIT, 'packages/create-plugin/template'));
	const configuration = [
		'plugin-cli',
		'create-plugin',
		'mock-host',
		'plugin-types',
	].map((name) => join(KIT, 'packages', name, 'package.json'));
	expect(await namingTheEngine([...template, ...configuration])).toEqual([]);
});

test('a generated project and everything the CLI prints don’t name the engine', async () => {
	await inTemporaryFolder(async (folder) => {
		const printed = [
			await succeed('create-noura-plugin', ['word-count'], folder),
		];
		const project = join(folder, 'word-count');
		expect(await namingTheEngine(await files(project))).toEqual([]);

		printed.push((await run('noura-plugin', ['--help'], project)).output);
		printed.push(await succeed('noura-plugin', ['build'], project));
		printed.push(await succeed('noura-plugin', ['package'], project));
		const dev = start('noura-plugin', ['dev'], project);
		await dev.waitFor(/^Waiting for changes/);
		await dev.stop();
		printed.push(dev.output());

		// Failed builds print what the tools said, which must not name it
		// either.
		const source = join(project, 'src/plugin.ts');
		const text = await readFile(source, 'utf8');
		await writeFile(source, `${text}\nexport const broken = ;\n`);
		printed.push((await run('noura-plugin', ['build'], project)).output);
		await writeFile(source, `${text}\nthrow new Error('at start');\n`);
		printed.push((await run('noura-plugin', ['build'], project)).output);

		for (const output of printed) {
			expect(output).not.toMatch(ENGINE);
		}
	});
});
