// Seam 5: the CLI's component compiles in the real host, which runs it
// with only the capabilities its manifest declares.
import { expect, test } from 'vitest';

import { buildTestPlugin, inTemporaryFolder } from './support/kit.js';
import { RealHost } from './support/real-host.js';

async function probe(host: RealHost, call: string): Promise<unknown> {
	const answer = await host.callTool('probe', 'probe', { call });
	if (!('answer' in answer)) {
		throw new Error(`The probe failed: ${answer.failure}`);
	}
	return JSON.parse(answer.answer);
}

test('the plugin runs with only the capabilities its manifest declares', async () => {
	await inTemporaryFolder(async (folder) => {
		const project = await buildTestPlugin('probe', folder);
		const host = await RealHost.start({ 'Plan.md': 'plan' });
		if (!host) {
			return;
		}
		try {
			expect(await host.install(project)).toMatchObject({
				id: 'probe',
				running: true,
				problem: null,
				approved: ['views', 'tools'],
			});

			// Each call the manifest doesn't cover is refused and names
			// the capability, and the clock needs none.
			expect(await probe(host, 'read-file')).toEqual({
				error: { tag: 'capability-not-declared', val: 'workspace-read' },
			});
			expect(await probe(host, 'fetch')).toEqual({
				error: { tag: 'capability-not-declared', val: 'network' },
			});
			expect(await probe(host, 'call-command')).toEqual({
				error: { tag: 'capability-not-declared', val: 'commands' },
			});
			expect(await probe(host, 'create-object')).toEqual({
				error: { tag: 'capability-not-declared', val: 'object-types' },
			});
			expect(await probe(host, 'now')).toEqual({ ok: true });
		} finally {
			await host.close();
		}
	});
});

test('a capability the manifest declares works', async () => {
	await inTemporaryFolder(async (folder) => {
		const project = await buildTestPlugin('probe', folder, [
			'name = "views"',
			'name = "tools"',
			'name = "workspace-read"',
		]);
		const host = await RealHost.start({ 'Plan.md': 'plan', 'Notes/Idea.md': 'idea' });
		if (!host) {
			return;
		}
		try {
			expect(await host.install(project)).toMatchObject({
				approved: ['views', 'tools', 'workspace-read'],
			});
			expect(await probe(host, 'read-file')).toEqual({ ok: 'plan' });
			expect(await probe(host, 'list-entries')).toEqual({
				ok: ['file Notes/Idea.md', 'file Plan.md', 'folder Notes'],
			});
		} finally {
			await host.close();
		}
	});
});
