// Seam 5: one test file runs a plugin on the mock host and on the real
// host. Each step expects the same result on both, denials included, and
// each denial names the capability the manifest doesn't declare.
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';

import { type Host, mockHost, realHost } from './support/hosts.js';
import { buildTestPlugin } from './support/kit.js';

const FILES = { 'Plan.md': 'plan', 'Notes/Idea.md': 'idea' };

/** The hosts each step runs on. The real host drops out when it isn't built. */
const HOSTS: [string, (folder: string) => Promise<Host | undefined>][] = [
	['the mock host', (folder) => mockHost(folder, FILES)],
	['the real host', (folder) => realHost(folder, FILES)],
];

/** A folder that lasts for one `describe` block. */
function folderForBlock(): { path: () => string } {
	let path = '';
	beforeAll(async () => {
		path = await mkdtemp(join(tmpdir(), 'noura-kit-'));
	});
	afterAll(async () => rm(path, { recursive: true, force: true }));
	return { path: () => path };
}

async function probe(host: Host, call: string): Promise<unknown> {
	const answer = await host.callTool('probe', { call });
	return 'answer' in answer ? JSON.parse(answer.answer) : answer;
}

const denied = (capability: string) => ({
	error: { tag: 'capability-not-declared', val: capability },
});

describe.each(HOSTS)(
	'a plugin that declares only views and tools, on %s',
	(_name, start) => {
		const folder = folderForBlock();
		let host: Host | undefined;
		beforeAll(async () => {
			host = await start(await buildTestPlugin('probe', folder.path()));
		});
		afterAll(async () => host?.close());

		test('shows its page', async (context) => {
			if (!host) return context.skip();
			expect(await host.pages()).toEqual([{ id: 'probe', title: 'Probe' }]);
			expect(await host.view('probe')).toEqual({
				id: 'root',
				kind: { tag: 'panel', val: 'Probe' },
				children: [
					{
						id: 'last',
						kind: { tag: 'text', val: 'No calls yet.' },
						children: [],
					},
					{
						id: 'read-file',
						kind: { tag: 'button', val: 'Read a file' },
						children: [],
					},
					{
						id: 'open-page',
						kind: { tag: 'button', val: 'Open this page' },
						children: [],
					},
				],
			});
		});

		test('a press that reads a file is refused, naming workspace-read', async (context) => {
			if (!host) return context.skip();
			await host.send('probe', { tag: 'pressed', val: 'read-file' });
			expect((await host.view('probe')).children[0]?.kind).toEqual({
				tag: 'text',
				val: 'read-file: {"error":{"tag":"capability-not-declared","val":"workspace-read"}}',
			});
		});

		test('a press may open a page, which views covers', async (context) => {
			if (!host) return context.skip();
			await host.send('probe', { tag: 'pressed', val: 'open-page' });
			expect((await host.view('probe')).children[0]?.kind).toEqual({
				tag: 'text',
				val: 'open-page: {"ok":null}',
			});
		});

		test('offers its tool', async (context) => {
			if (!host) return context.skip();
			expect(await host.tools()).toEqual([
				{
					name: 'probe',
					description:
						'Calls one capability interface and reports what came back.',
					inputSchema:
						'{"type":"object","properties":{"call":{"type":"string"}},"required":["call"]}',
				},
			]);
		});

		test('each call it didn’t declare is refused and names the capability', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'read-file')).toEqual(denied('workspace-read'));
			expect(await probe(host, 'list-entries')).toEqual(
				denied('workspace-read'),
			);
			expect(await probe(host, 'fetch')).toEqual(denied('network'));
			expect(await probe(host, 'call-command')).toEqual(denied('commands'));
			expect(await probe(host, 'create-object')).toEqual(
				denied('object-types'),
			);
			expect(await probe(host, 'list-objects')).toEqual(denied('object-types'));
		});

		test('the clock and the console need no capability, and a tool can’t open a page', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'now')).toEqual({ ok: true });
			expect(await probe(host, 'console')).toEqual({ ok: null });
			expect(await probe(host, 'open-page')).toEqual({
				error: {
					tag: 'failed',
					val: 'Noura opens things only in answer to something you did.',
				},
			});
		});

		test('a tool’s own failure comes back with its message', async (context) => {
			if (!host) return context.skip();
			expect(await host.callTool('probe', { call: 'fail' })).toEqual({
				failure: 'The probe failed on purpose.',
			});
		});
	},
);

describe.each(HOSTS)(
	'a plugin that also declares workspace-read, on %s',
	(_name, start) => {
		const folder = folderForBlock();
		let host: Host | undefined;
		beforeAll(async () => {
			host = await start(
				await buildTestPlugin('probe', folder.path(), [
					'name = "views"',
					'name = "tools"',
					'name = "workspace-read"',
				]),
			);
		});
		afterAll(async () => host?.close());

		test('reads the workspace', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'read-file')).toEqual({ ok: 'plan' });
			expect(await probe(host, 'list-entries')).toEqual({
				ok: ['file Notes/Idea.md', 'file Plan.md', 'folder Notes'],
			});
		});

		test('a missing file and a path outside the workspace fail as Noura says', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'read-missing')).toEqual({
				error: { tag: 'not-found', val: 'Missing.md' },
			});
			expect(await probe(host, 'read-outside')).toEqual({
				error: { tag: 'invalid-path', val: '../Outside.md' },
			});
		});

		test('the capabilities it still didn’t declare stay refused', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'fetch')).toEqual(denied('network'));
			expect(await probe(host, 'create-object')).toEqual(
				denied('object-types'),
			);
		});
	},
);

describe.each(HOSTS)(
	'a plugin that declares every capability it calls, on %s',
	(_name, start) => {
		const folder = folderForBlock();
		let host: Host | undefined;
		beforeAll(async () => {
			host = await start(
				await buildTestPlugin('probe', folder.path(), [
					'name = "views"',
					'name = "tools"',
					'name = "object-types"',
					'name = "network"\nhosts = ["api.example.com"]',
					'name = "commands"\ntargets = ["tasks/create-task"]',
				]),
			);
		});
		afterAll(async () => host?.close());

		test('writes objects of its own type, and only those', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'create-object')).toEqual({ ok: true });
			expect(await probe(host, 'list-own')).toEqual({
				ok: ['probe-notes/Probe.md'],
			});
			expect(await probe(host, 'update-stale')).toMatchObject({
				error: { tag: 'revision-conflict' },
			});
			expect(await probe(host, 'list-objects')).toEqual(denied('object-types'));
		});

		test('reaches only the HTTPS hosts it lists', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'fetch')).toMatchObject({
				error: { tag: 'network-refused' },
			});
			expect(await probe(host, 'fetch-http')).toMatchObject({
				error: { tag: 'network-refused' },
			});
		});

		test('calls only the commands it lists, of plugins that run', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'call-command')).toEqual({
				error: { tag: 'target-unavailable', val: 'tasks/create-task' },
			});
			expect(await probe(host, 'call-other')).toEqual({
				error: { tag: 'target-not-declared', val: 'calendar/show-day' },
			});
		});

		test('still reads nothing without workspace-read', async (context) => {
			if (!host) return context.skip();
			expect(await probe(host, 'read-file')).toEqual(denied('workspace-read'));
		});
	},
);
