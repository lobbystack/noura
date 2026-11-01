// Seam 5: every interface of noura:plugin has an author page in
// docs/plugins/interfaces with an example, and each example's manifest and
// source type-check, build into a plugin and run in the real host. The
// author guide's examples do too: its plugin builds with its manifest and
// runs in the real host, and its test runs against the mock host on the
// template's plugin, as in a new project.
import { spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';

import { KIT, succeed } from './support/kit.js';
import { RealHost, type ViewAnswer, texts } from './support/real-host.js';

const REPO = join(KIT, '../..');
const GUIDE = join(REPO, 'docs/plugins/README.md');
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

/** The bodies of the fenced blocks of `language` in `markdown`, in order. */
function blocks(markdown: string, language: string): string[] {
	return [
		...markdown.matchAll(
			new RegExp(`^\`\`\`${language}\\n([\\s\\S]*?)^\`\`\`$`, 'gm'),
		),
	].map((match) => match[1] ?? '');
}

/** The body of the first fenced block of `language` in `markdown`. */
function block(markdown: string, language: string): string | undefined {
	return blocks(markdown, language)[0];
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

/** The guide's examples: its manifest, and its TypeScript blocks, which are plugins or tests. */
const guide = await (async () => {
	const markdown = await readFile(GUIDE, 'utf8');
	const typescript = blocks(markdown, 'ts');
	const isTest = (source: string) => /from 'vitest'/.test(source);
	return {
		manifest: block(markdown, 'toml'),
		plugins: typescript.filter((source) => !isTest(source)),
		tests: typescript.filter(isTest),
	};
})();

/** The project folder of the guide's `index`th plugin or test. */
const guideProject = (kind: 'plugin' | 'test', index: number) =>
	join(EXAMPLES, `guide-${kind}-${index + 1}`);

/**
 * Write the guide's examples into project folders: each plugin with the
 * guide's manifest, and each test in a project that create-noura-plugin
 * writes, in place of the template's test.
 */
async function writeGuide(): Promise<void> {
	if (!guide.manifest) {
		throw new Error('The guide needs a toml block with plugin.toml.');
	}
	for (const [index, source] of guide.plugins.entries()) {
		const project = guideProject('plugin', index);
		await mkdir(join(project, 'src'), { recursive: true });
		await writeFile(join(project, 'plugin.toml'), guide.manifest);
		await writeFile(join(project, 'src/plugin.ts'), source);
	}
	for (const [index, source] of guide.tests.entries()) {
		const project = guideProject('test', index);
		await succeed('create-noura-plugin', [project], EXAMPLES);
		await writeFile(join(project, 'test/plugin.test.ts'), source);
		// The kit's own Vitest configuration doesn't cover the project.
		await writeFile(
			join(project, 'vitest.config.mjs'),
			"export default { test: { include: ['test/**/*.test.ts'] } };\n",
		);
	}
}

test('the guide shows a plugin and a test', () => {
	expect(guide.plugins.length).toBeGreaterThan(0);
	expect(guide.tests.length).toBeGreaterThan(0);
});

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
		await writeGuide();
		await writeFile(
			join(EXAMPLES, 'tsconfig.json'),
			JSON.stringify({
				extends: '../packages/create-plugin/template/tsconfig.json',
				include: ['*/src', '*/test'],
			}),
		);
		host = await RealHost.start({
			'Plan.md': '# Plan\n',
			'Notes/Idea.md': 'An idea.\n',
			'Notes/Kept.md':
				'---\nid: note_01j9qv6m0000000000000000aa\ntype: note\n---\nKept.\n',
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

	test.each(guide.plugins.map((_, index) => index + 1))(
		'the guide’s plugin %i builds and runs in Noura',
		async (number) => {
			const project = guideProject('plugin', number - 1);

			await succeed('noura-plugin', ['build'], project);

			if (!host) {
				return;
			}
			const installed = await host.install(project);
			expect(installed).toMatchObject({ running: true, problem: null });
			const id = 'id' in installed ? installed.id : '';
			const shown = (await host.pages()).filter((page) =>
				page.id.startsWith(`plugin:${id}:`),
			);
			expect(shown).not.toEqual([]);
			for (const page of shown) {
				const answer: ViewAnswer = await host.view(page.id);
				expect(answer, page.id).toHaveProperty('view');
				if (answer && 'view' in answer) {
					expect(texts(answer.view), page.id).not.toEqual([]);
				}
			}
		},
	);

	test.each(guide.tests.map((_, index) => index + 1))(
		'the guide’s test %i passes on the mock host',
		async (number) => {
			const project = guideProject('test', number - 1);
			await succeed('noura-plugin', ['build'], project);
			const vitest = join(
				dirname(createRequire(import.meta.url).resolve('vitest/package.json')),
				'vitest.mjs',
			);
			// Without the settings this Vitest run gives its workers.
			const env = Object.fromEntries(
				Object.entries(process.env).filter(
					([name]) => !name.startsWith('VITEST'),
				),
			);
			const ran = spawnSync(process.execPath, [vitest, 'run'], {
				cwd: project,
				encoding: 'utf8',
				env: { ...env, NO_COLOR: '1', FORCE_COLOR: '0' },
			});
			expect(ran.status, ran.stdout + ran.stderr).toBe(0);
			expect(ran.stdout).toMatch(/Tests +\d+ passed/);
		},
	);

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
