// Seam 5: a plugin built against noura:plugin@0.1.0 builds and runs in the
// real host on every change, so a newer interface version that breaks it
// fails here first.
import { expect, test } from 'vitest';

import { buildTestPlugin, inTemporaryFolder } from './support/kit.js';
import { RealHost } from './support/real-host.js';

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
				view: { kind: { tag: 'text', val: 'Built against noura:plugin@0.1.0.' } },
			});
			const answer = await host.callTool('pinned', 'check', {});
			expect('answer' in answer && JSON.parse(answer.answer)).toEqual({
				entries: 1,
				plan: 'plan',
				pins: 0,
				clock: true,
			});
		} finally {
			await host.close();
		}
	});
});
