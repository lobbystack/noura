// The build writes the component where the manifest's `entry` says, so an
// entry that names the plugin's own files must never get written over.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { inTemporaryFolder, run, succeed } from './support/kit.js';

/** Write a new project whose manifest's entry is `entry`. */
async function projectWithEntry(folder: string, entry: string) {
	await succeed('create-noura-plugin', ['word-count'], folder);
	const project = join(folder, 'word-count');
	const manifest = join(project, 'plugin.toml');
	const text = await readFile(manifest, 'utf8');
	await writeFile(
		manifest,
		text.replace('entry = "dist/plugin.wasm"', `entry = "${entry}"`),
	);
	return project;
}

/** Every file of the project the build could write over, with its bytes. */
async function kept(project: string, paths: string[]) {
	return Object.fromEntries(
		await Promise.all(
			paths.map(async (path) => [path, await readFile(join(project, path))]),
		),
	);
}

// What the build says when it refuses an entry.
const NOT_WASM = "doesn't end in .wasm.";
const IN_SRC = "is inside src, which holds the plugin's source.";

test.each([
	['src/plugin.ts', NOT_WASM],
	['plugin.toml', NOT_WASM],
	['src', NOT_WASM],
	['src/plugin.wasm', IN_SRC],
	// The same folder as src on a file system that ignores case.
	['SRC/plugin.wasm', IN_SRC],
])(
	'build and package refuse the entry %s and leave the source and manifest as they were',
	async (entry, refusal) => {
		await inTemporaryFolder(async (folder) => {
			const project = await projectWithEntry(folder, entry);
			const files = ['src/plugin.ts', 'plugin.toml', 'test/plugin.test.ts'];
			const before = await kept(project, files);

			for (const command of ['build', 'package']) {
				const ran = await run('noura-plugin', [command], project);
				expect(ran.code, ran.output).toBe(1);
				expect(ran.output).toContain(
					`The entry ${entry} in plugin.toml ${refusal}`,
				);
				expect(ran.output).not.toMatch(/^\s+at /m);
			}

			expect(await kept(project, files)).toEqual(before);
		});
	},
);

test('the build won’t write over a file at the entry that isn’t a component', async () => {
	await inTemporaryFolder(async (folder) => {
		const project = await projectWithEntry(folder, 'dist/notes.wasm');
		await mkdir(join(project, 'dist'));
		const notes = join(project, 'dist/notes.wasm');
		await writeFile(notes, 'Notes I keep next to the build.\n');

		for (const command of ['build', 'package']) {
			const ran = await run('noura-plugin', [command], project);
			expect(ran.code, ran.output).toBe(1);
			expect(ran.output).toContain(
				"The entry dist/notes.wasm in plugin.toml names a file that isn't a component, so the build won't write over it.",
			);
		}
		expect(await readFile(notes, 'utf8')).toBe(
			'Notes I keep next to the build.\n',
		);
	});
});

test('the build writes over its own earlier component and over an empty file', async () => {
	await inTemporaryFolder(async (folder) => {
		const project = await projectWithEntry(folder, 'dist/plugin.wasm');
		await mkdir(join(project, 'dist'));
		const component = join(project, 'dist/plugin.wasm');
		await writeFile(component, '');

		await succeed('noura-plugin', ['build'], project);
		const first = await readFile(component);
		expect(first.subarray(0, 4)).toEqual(Buffer.from([0, 0x61, 0x73, 0x6d]));

		await succeed('noura-plugin', ['package'], project);
		expect((await readFile(component)).subarray(0, 4)).toEqual(
			first.subarray(0, 4),
		);
	});
});
