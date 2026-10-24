// A test plugin that calls each capability interface on request and
// reports what came back, so a test can compare the mock host with the
// real host. Its `probe` tool takes `{"call": name}` and answers
// `{"ok": ...}` or `{"error": ...}` as JSON. Its page shows the outcome of
// the last call a button made.
import type {
	ObjectTypesExports,
	ToolsExports,
	ViewExports,
} from '@noura/plugin-types';
import { call as callCommand } from 'noura:plugin/command-calls@0.1.0';
import { now } from 'noura:plugin/clock@0.1.0';
import { openPage } from 'noura:plugin/navigation@0.1.0';
import { fetch } from 'noura:plugin/network@0.1.0';
import { create, list, update } from 'noura:plugin/objects@0.1.0';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';
import { listEntries, readFile } from 'noura:plugin/workspace@0.1.0';

/** What each call does, with a summary of its answer that is the same on any host. */
const CALLS: Record<string, () => unknown> = {
	'list-entries': () =>
		listEntries()
			.map((entry) =>
				entry.tag === 'ok'
					? `${entry.val.kind} ${entry.val.path}`
					: entry.val.tag,
			)
			.sort(),
	'read-file': () => new TextDecoder().decode(readFile('Plan.md').bytes),
	'read-missing': () => readFile('Missing.md').path,
	'read-outside': () => readFile('../Outside.md').path,
	fetch: () => get('https://example.com/'),
	'fetch-http': () => get('http://api.example.com/'),
	'fetch-listed': () => get('https://api.example.com/weather'),
	'clock-zone': () => now().timeZone,
	'call-command': () => callCommand('tasks', 'create-task', '{}'),
	'call-other': () => callCommand('calendar', 'show-day', '{}'),
	'create-object': () =>
		create({
			objectType: 'probe-note',
			title: 'Probe',
			body: '',
			properties: '{}',
		}).id.startsWith('probe-note_'),
	'list-objects': () => list('note').length,
	'list-own': () => list('probe-note').map((object) => object.path),
	'update-stale': () => {
		const [object] = list('probe-note');
		return (
			object && update(object.id, 'stale', { properties: '{}', remove: [] }).id
		);
	},
	now: () => /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z$/.test(now().instant),
	'open-page': () => openPage('probe'),
	// Noura gives plugins no console: what they write there goes nowhere.
	console: () => console.log('The probe wrote to the console.'),
};

function get(url: string): number {
	return fetch({ method: 'get', url, headers: [], body: new Uint8Array() })
		.status;
}

function attempt(name: string): { ok: unknown } | { error: unknown } {
	const call = CALLS[name];
	if (!call) {
		return { error: `There's no call ${name}.` };
	}
	try {
		return { ok: call() ?? null };
	} catch (error) {
		// A refused or failed call throws an error whose payload is the
		// `error` variant of noura:plugin/types.
		return { error: (error as { payload?: unknown }).payload ?? String(error) };
	}
}

let last = 'No calls yet.';

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

export const view: ViewExports = {
	pages: () => [{ id: 'probe', title: 'Probe' }],
	render: () => [
		node('root', { tag: 'panel', val: 'Probe' }, [1, 2, 3]),
		node('last', { tag: 'text', val: last }),
		node('read-file', { tag: 'button', val: 'Read a file' }),
		node('open-page', { tag: 'button', val: 'Open this page' }),
	],
	handleEvent(_page: string, event: Event) {
		if (event.tag === 'pressed') {
			last = `${event.val}: ${JSON.stringify(attempt(event.val))}`;
		}
	},
};

// Noura asks for these only when the manifest declares object-types.
export const objectTypes: ObjectTypesExports = {
	types: () => [{ name: 'probe-note', schema: '{"properties":[]}' }],
};

export const tools: ToolsExports = {
	tools: () => [
		{
			name: 'probe',
			description: 'Calls one capability interface and reports what came back.',
			inputSchema: JSON.stringify({
				type: 'object',
				properties: { call: { type: 'string' } },
				required: ['call'],
			}),
		},
	],
	callTool(name: string, args: string) {
		if (name !== 'probe') {
			throw `There's no tool ${name}.`;
		}
		const { call } = JSON.parse(args) as { call: string };
		if (call === 'fail') {
			// The tool's own failure, which goes back to the assistant.
			throw 'The probe failed on purpose.';
		}
		return JSON.stringify(attempt(call));
	},
};
