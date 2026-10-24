// Seam 5: every interface of noura:plugin has an author page in
// docs/plugins/interfaces with an example, and each example's manifest and
// source type-check, build into a plugin and run in the real host.
import { spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';

import { KIT, succeed } from './support/kit.js';
import { RealHost } from './support/real-host.js';

const REPO = join(KIT, '../..');
const PAGES = join(REPO, 'docs/plugins/interfaces');
const WIT = join(REPO, 'crates/plugin-sdk/wit/plugin.wit');
/** Inside the kit, so the examples resolve its packages as a project does. */
const EXAMPLES = join(KIT, '.examples');

/** The interfaces the WIT defines, by name. */
async function interfaces(): Promise<string[]> {
	const wit = await readFile(WIT, 'utf8');
	return [...wit.matchAll(/^interface ([a-z-]+) \{/gm)]
		.map((match) => match[1] ?? '')
		.sort();
}

/** The body of the first fenced block of `language` in `markdown`. */
function block(markdown: string, language: string): string | undefined {
	return new RegExp(`^\`\`\`${language}\\n([\\s\\S]*?)^\`\`\`$`, 'm').exec(
		markdown,
	)?.[1];
}

const pages = (await readdir(PAGES))
	.filter((name) => name.endsWith('.md'))
	.sort();

/** Write the example on `page` into its own project folder. */
async function writeExample(page: string): Promise<string> {
	const markdown = await readFile(join(PAGES, page), 'utf8');
	const manifest = block(markdown, 'toml');
	const source = block(markdown, 'ts');
	if (manifest === undefined || source === undefined) {
		throw new Error(
			`${page} needs a toml block with plugin.toml and a ts block with src/plugin.ts.`,
		);
	}
	const project = join(EXAMPLES, page.replace(/\.md$/, ''));
	await mkdir(join(project, 'src'), { recursive: true });
	await writeFile(join(project, 'plugin.toml'), manifest);
	await writeFile(join(project, 'src/plugin.ts'), source);
	return project;
}

test('each interface has an author page', async () => {
	expect(pages.map((page) => page.replace(/\.md$/, ''))).toEqual(
		await interfaces(),
	);
});

describe('each page’s example', () => {
	let host: RealHost | undefined;
	beforeAll(async () => {
		await rm(EXAMPLES, { recursive: true, force: true });
		for (const page of pages) {
			await writeExample(page);
		}
		await writeFile(
			join(EXAMPLES, 'tsconfig.json'),
			JSON.stringify({
				extends: '../packages/create-plugin/template/tsconfig.json',
				include: ['*/src'],
			}),
		);
		host = await RealHost.start({
			'Plan.md': '# Plan\n',
			'Notes/Idea.md': 'An idea.\n',
		});
	});
	afterAll(async () => {
		await host?.close();
		await rm(EXAMPLES, { recursive: true, force: true });
	});

	test('type-checks', () => {
		const typescript = dirname(
			createRequire(import.meta.url).resolve('typescript/package.json'),
		);
		const tsc = join(typescript, 'bin/tsc');
		const checked = spawnSync(
			process.execPath,
			[tsc, '-p', join(EXAMPLES, 'tsconfig.json')],
			{
				encoding: 'utf8',
			},
		);
		expect(checked.stdout + checked.stderr).toBe('');
		expect(checked.status).toBe(0);
	});

	test.each(pages)('in %s builds and runs in Noura', async (page) => {
		const project = join(EXAMPLES, page.replace(/\.md$/, ''));

		await succeed('noura-plugin', ['build'], project);

		if (!host) {
			return;
		}
		const installed = await host.install(project);
		expect(installed).toMatchObject({ running: true, problem: null });
		const id = 'id' in installed ? installed.id : '';
		for (const shown of await host.pages()) {
			if (shown.id.startsWith(`plugin:${id}:`)) {
				expect(await host.view(shown.id), shown.id).toHaveProperty('view');
			}
		}
	});
});
