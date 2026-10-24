// The mock host's own answers, which a test passes in, and what it tells
// an author whose test calls something the manifest doesn't declare.
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
	CapabilityError,
	type MockHost,
	createMockHost,
} from '@noura/mock-host';
import { afterAll, beforeAll, expect, test } from 'vitest';

import { buildTestPlugin } from './support/kit.js';

let folder = '';
let project = '';
let host: MockHost;

beforeAll(async () => {
	folder = await mkdtemp(join(tmpdir(), 'noura-kit-'));
	project = await buildTestPlugin('probe', folder, [
		'name = "views"',
		'name = "tools"',
		'name = "object-types"',
		'name = "network"\nhosts = ["api.example.com"]',
		'name = "commands"\ntargets = ["tasks/create-task"]',
	]);
	host = await createMockHost({
		folder: project,
		clock: () => ({
			instant: '2026-10-21T13:30:00Z',
			timeZone: 'America/Toronto',
		}),
		fetch: (request) => ({
			status: 200,
			url: request.url,
			headers: [],
			body: new Uint8Array(),
		}),
		commands: { 'tasks/create-task': (input) => `created from ${input}` },
	});
});

afterAll(async () => {
	await host.close();
	await rm(folder, { recursive: true, force: true });
});

async function probe(call: string): Promise<unknown> {
	return JSON.parse(await host.callTool('probe', { call }));
}

test('requests to a listed host reach the fetch the test passes', async () => {
	expect(await probe('fetch-listed')).toEqual({ ok: 200 });
});

test('calls to a listed command reach the commands the test passes', async () => {
	expect(await probe('call-command')).toEqual({ ok: 'created from {}' });
});

test('the clock reads what the test passes', async () => {
	expect(await probe('clock-zone')).toEqual({ ok: 'America/Toronto' });
});

test('the objects the plugin writes show in the mock workspace', async () => {
	expect(await host.objectTypes()).toEqual(['probe-note']);
	await probe('create-object');
	expect(
		host.objects().map((object) => [object.objectType, object.path]),
	).toContainEqual(['probe-note', 'probe-notes/Probe.md']);
});

test('a press that opens a page shows in the openings', async () => {
	await host.press('probe', 'open-page');
	expect(host.openings).toEqual([{ kind: 'page', page: 'probe' }]);
});

test('a view the manifest doesn’t declare names the capability', async () => {
	const withoutViews = await mkdtemp(join(tmpdir(), 'noura-kit-'));
	try {
		const tools = await buildTestPlugin('probe', withoutViews, [
			'name = "tools"',
		]);
		const toolsOnly = await createMockHost({ folder: tools });
		try {
			expect(await toolsOnly.pages()).toEqual([]);
			const refused = await toolsOnly
				.view('probe')
				.catch((error: unknown) => error);
			expect(refused).toBeInstanceOf(CapabilityError);
			expect((refused as CapabilityError).capability).toBe('views');
		} finally {
			await toolsOnly.close();
		}
	} finally {
		await rm(withoutViews, { recursive: true, force: true });
	}
});
