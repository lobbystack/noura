// @noura/plugin-types declares every interface version the kit builds, so
// a plugin built against noura:plugin@0.1.0 keeps its types after Noura's
// interface moves on. The test stands in a 0.2.0 for the current WIT and
// type-checks the pinned 0.1.0 plugin and a 0.2.0 one against the result.
import { spawnSync } from 'node:child_process';
import { cp, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { expect, test } from 'vitest';

import { KIT, inTemporaryFolder } from './support/kit.js';

const LIVE_WIT = join(KIT, '../../crates/plugin-sdk/wit');
const TYPES = join(KIT, 'packages/plugin-types');

/** Run Node with `args` and fail with its output unless it succeeds. */
function node(args: string[], cwd: string): string {
	const ran = spawnSync(process.execPath, args, { cwd, encoding: 'utf8' });
	const output = ran.stdout + ran.stderr;
	if (ran.status !== 0) {
		throw new Error(
			`node ${args.join(' ')} stopped with ${ran.status}:\n${output}`,
		);
	}
	return output;
}

/** A copy of plugin-sdk's WIT that declares `version` instead of its own. */
async function witAt(folder: string, version: string): Promise<string> {
	const live = await readFile(join(LIVE_WIT, 'plugin.wit'), 'utf8');
	const current = /^package noura:plugin@([0-9.]+);/m.exec(live)?.[1] ?? '';
	await mkdir(folder, { recursive: true });
	for (const name of await readdir(LIVE_WIT)) {
		const text = await readFile(join(LIVE_WIT, name), 'utf8');
		await writeFile(
			join(folder, name),
			text.replaceAll(`@${current}`, `@${version}`),
		);
	}
	return folder;
}

test('after a new interface version, a 0.1.0 plugin keeps its types and a new one gets the new version’s', async () => {
	await inTemporaryFolder(async (folder) => {
		const next = await witAt(join(folder, 'wit'), '0.2.0');
		const project = join(folder, 'project');
		const types = join(project, 'node_modules/@noura/plugin-types');
		await mkdir(types, { recursive: true });
		for (const file of ['package.json', 'index.d.ts']) {
			await cp(join(TYPES, file), join(types, file));
		}
		node(
			[join(TYPES, 'scripts/generate.mjs'), '--wit', next, '--out', types],
			KIT,
		);
		expect((await readdir(join(types, 'generated'))).sort()).toEqual([
			'0.1.0',
			'0.2.0',
			'versions.d.ts',
		]);

		await cp(
			join(KIT, 'packages/create-plugin/template/tsconfig.json'),
			join(project, 'tsconfig.json'),
		);
		await cp(
			join(KIT, 'tests/plugins/pinned-0.1.0/src/plugin.ts'),
			join(project, 'src/pinned.ts'),
		);
		await writeFile(
			join(project, 'src/next.ts'),
			`import type { ViewExports } from '@noura/plugin-types/0.2.0';
import type { Node } from 'noura:plugin/ui@0.2.0';
import { listEntries } from 'noura:plugin/workspace@0.2.0';

export const view: ViewExports = {
	pages: () => [{ id: 'next', title: 'Next' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'text', val: \`\${listEntries().length} entries\` },
			children: new Uint32Array(),
		},
	],
};
`,
		);
		const typescript = dirname(
			createRequire(import.meta.url).resolve('typescript/package.json'),
		);
		expect(node([join(typescript, 'bin/tsc'), '-p', project], project)).toBe(
			'',
		);
	});
});
