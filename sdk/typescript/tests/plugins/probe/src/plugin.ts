// A test plugin that calls each capability interface on request and
// reports what came back, so a test can compare the mock host with the
// real host. Its `probe` tool takes `{"call": name}` and answers
// `{"ok": ...}` or `{"error": ...}` as JSON. Its page shows the outcome of
// the last call a button made.
import type {
	ObjectTypesExports,
	ToolsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import {
	chats,
	createChat,
	providers,
	readChat,
	readRun,
	renameChat,
	resume,
	runTools,
	send,
	setRetention,
	webFetch,
	webReady,
	webSearch,
	workspaceTools,
} from 'noura:plugin/ai@0.1.0';
import { call as callCommand } from 'noura:plugin/command-calls@0.1.0';
import { now } from 'noura:plugin/clock@0.1.0';
import { openPage } from 'noura:plugin/navigation@0.1.0';
import { fetch } from 'noura:plugin/network@0.1.0';
import { create, list, update } from 'noura:plugin/objects@0.1.0';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';
import {
	type Object as WorkspaceObject,
	listEntries,
	readFile,
	readObject,
	readObjectAt,
} from 'noura:plugin/workspace@0.1.0';

/** The note that `Shared/Kept.md` holds, and no other file. */
const KEPT = 'note_01j9qv6m0000000000000000aa';
/** The note that `Shared/Draft.md` and its copy both hold. */
const SHARED = 'note_01j9qv6m0000000000000000bb';

/** The properties the probe writes into a recipe, of each kind a property holds. */
const RECIPE = {
	tags: ['quick', 'spicy'],
	cuisine: 'Thai',
	rating: 4.5,
	servings: 2,
	vegetarian: false,
	source: null,
	steps: 'Chop.\nFry.',
	code: '0123',
	answer: 'yes',
	empty: '',
	timing: { prep: 10, cook: 'one hour: or so' },
};

/** An object without its revision, whose form differs from host to host. */
function summary(object: WorkspaceObject) {
	const { id, objectType, title, path, body, properties } = object;
	return { id, objectType, title, path, body, properties };
}

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
	'list-object-entries': () =>
		listEntries()
			.flatMap((entry) =>
				entry.tag === 'ok' && entry.val.objectId
					? [`${entry.val.path} ${entry.val.objectType} ${entry.val.objectId}`]
					: [],
			)
			.sort(),
	'read-object': () => summary(readObject(KEPT)),
	'read-object-shared': () => summary(readObject(SHARED)),
	'read-object-missing': () =>
		summary(readObject('note_01j9qv6m0000000000000000cc')),
	'read-object-at': () => summary(readObjectAt('Shared/Draft copy.md')),
	'read-object-at-plain': () => summary(readObjectAt('Plan.md')),
	'read-object-at-missing': () => summary(readObjectAt('Missing.md')),
	'read-object-at-outside': () => summary(readObjectAt('../Outside.md')),
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
	'read-own-file': () => {
		const object = create({
			objectType: 'probe-note',
			title: 'Recipe',
			body: 'The recipe.\n',
			properties: JSON.stringify(RECIPE),
		});
		// The ID and the time differ from host to host, and their form doesn't.
		return new TextDecoder()
			.decode(readFile(object.path).bytes)
			.replace(`id: ${object.id}\n`, 'id: <id>\n')
			.replace(
				/^(created|updated): \d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,9})?Z$/gm,
				'$1: <instant>',
			);
	},
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
	'ai-providers': () => providers(),
	'ai-new-chat': () => {
		const chat = createChat();
		const [read, messages] = readChat(chat.id);
		return {
			id: read.id === chat.id && /^chat_[0-9a-z]{26}$/.test(chat.id),
			title: read.title,
			messages: messages.length,
			listed: chats().some((listed) => listed.id === chat.id),
		};
	},
	'ai-send-unknown': () =>
		send({
			chat: createChat().id,
			provider: 'missing',
			message: 'Hello.',
			tools: [],
		}).id,
	'ai-read-chat-missing': () => readChat('chat_01j9qv6m0000000000000000cc'),
	'ai-read-run-missing': () => readRun('run_missing'),
	'ai-rename-new-chat': () => renameChat(createChat().id, 'Garden', ''),
	'ai-rename-missing': () =>
		renameChat('chat_01j9qv6m0000000000000000cc', 'Garden', 'rev'),
	'ai-keep-new-chat': () => setRetention(createChat().id, 'ephemeral', ''),
	'ai-resume-new-chat': () => resume(createChat().id, 'missing', []),
	'ai-run-tools-missing': () => runTools('run_missing'),
	'ai-workspace-tools': () => workspaceTools().map((tool) => tool.name),
	'ai-web-ready': () => webReady(),
	'ai-web-search': () => webSearch('tomatoes', 3),
	'ai-web-fetch': () => webFetch('https://garden.example.com/'),
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
	// Noura writes the declared properties first, in this order.
	types: () => [
		{
			name: 'probe-note',
			schema: JSON.stringify({
				properties: [
					{ name: 'rating', kind: 'number' },
					{ name: 'cuisine', kind: 'string' },
				],
			}),
		},
	],
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
