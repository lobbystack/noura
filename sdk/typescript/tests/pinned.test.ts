// Seam 5: a plugin built against noura:plugin@0.1.0 builds and runs in the
// real host on every change, so a newer interface version that breaks it
// fails here first.
import { existsSync } from 'node:fs';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { KIT, buildTestPlugin, inTemporaryFolder } from './support/kit.js';
import { RealHost } from './support/real-host.js';

const LIVE_WIT = join(KIT, '../../crates/plugin-sdk/wit');
const FROZEN_WIT = join(KIT, 'packages/plugin-cli/wit-frozen');

/** The files under `folder`, by path inside it, with their text. */
async function tree(folder: string): Promise<Record<string, string>> {
	const files: Record<string, string> = {};
	for (const entry of await readdir(folder, {
		recursive: true,
		withFileTypes: true,
	})) {
		if (entry.isFile()) {
			const path = join(entry.parentPath, entry.name);
			files[path.slice(folder.length + 1).replaceAll('\\', '/')] =
				await readFile(path, 'utf8');
		}
	}
	return files;
}

test('the kit keeps a frozen copy of noura:plugin@0.1.0, and the copy of the current version matches plugin-sdk’s WIT', async () => {
	expect(existsSync(join(FROZEN_WIT, '0.1.0/plugin.wit'))).toBe(true);
	const live = await readFile(join(LIVE_WIT, 'plugin.wit'), 'utf8');
	const current = /^package noura:plugin@([0-9.]+);/m.exec(live)?.[1] ?? '';
	if (existsSync(join(FROZEN_WIT, current))) {
		expect(
			await tree(join(FROZEN_WIT, current)),
			`noura:plugin@${current} is frozen in ${FROZEN_WIT}, and crates/plugin-sdk/wit changed it. Move the change to a new interface version, or, while ${current} isn't released, run npm run freeze-wit -w @noura/plugin-cli to refresh the frozen copy.`,
		).toEqual(await tree(LIVE_WIT));
	}
});

test('a plugin built against noura:plugin@0.1.0 runs every interface in Noura', async () => {
	await inTemporaryFolder(async (folder) => {
		const project = await buildTestPlugin('pinned-0.1.0', folder);
		const host = await RealHost.start({ 'Plan.md': 'plan' });
		if (!host) {
			return;
		}
		try {
			expect(await host.install(project)).toMatchObject({
				id: 'pinned',
				running: true,
				problem: null,
				approved: [
					'workspace-read',
					'object-types',
					'views',
					'network',
					'commands',
					'inbox-sections',
					'settings',
					'tools',
				],
			});
			expect(await host.view('plugin:pinned:pinned')).toMatchObject({
				view: {
					kind: { tag: 'text', val: 'Built against noura:plugin@0.1.0.' },
				},
			});
			const answer = await host.callTool('pinned', 'check', {});
			expect('answer' in answer && JSON.parse(answer.answer)).toEqual({
				entries: 1,
				readBack: true,
				plan: 'plan',
				pins: 0,
				clock: true,
			});
		} finally {
			await host.close();
		}
	});
});
