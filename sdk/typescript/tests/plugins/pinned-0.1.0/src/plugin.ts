// A plugin that uses every function of noura:plugin@0.1.0: it imports each
// capability interface's functions and offers each export. Its `check`
// tool calls the imports that need no setup and reports what they gave.
import type {
	CommandsExports,
	InboxExports,
	ObjectTypesExports,
	SettingsExports,
	ToolsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import { now } from 'noura:plugin/clock@0.1.0';
import { call } from 'noura:plugin/command-calls@0.1.0';
import {
	openObject,
	openPage,
	trashObject,
} from 'noura:plugin/navigation@0.1.0';
import { fetch } from 'noura:plugin/network@0.1.0';
import {
	create,
	delete as remove,
	get,
	list,
	moveTo,
	update,
} from 'noura:plugin/objects@0.1.0';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';
import {
	listEntries,
	readFile,
	readObject,
	readObjectAt,
} from 'noura:plugin/workspace@0.1.0';

/** Every import, so a build against an interface that lost one fails. */
export const IMPORTS = [
	now,
	call,
	openObject,
	openPage,
	trashObject,
	fetch,
	create,
	remove,
	get,
	list,
	moveTo,
	update,
	listEntries,
	readFile,
	readObject,
	readObjectAt,
];

function text(id: string, value: string): Node[] {
	return [
		{ id, kind: { tag: 'text', val: value }, children: new Uint32Array() },
	];
}

function check(): string {
	const entries = listEntries().filter((entry) => entry.tag === 'ok').length;
	const created = create({
		objectType: 'pin',
		title: 'Pin',
		body: 'pinned',
		properties: '{}',
	});
	const moved = moveTo(created.id, created.revision, 'pins/Moved.md');
	const updated = update(moved.id, moved.revision, {
		body: 'still pinned',
		properties: '{}',
		remove: [],
	});
	const readBack =
		readObjectAt(updated.path).id === updated.id &&
		readObject(updated.id).path === updated.path;
	remove(updated.id, updated.revision);
	return JSON.stringify({
		entries,
		readBack,
		plan: new TextDecoder().decode(readFile('Plan.md').bytes),
		pins: list('pin').length,
		clock: now().instant.endsWith('Z'),
	});
}

export const view: ViewExports = {
	pages: () => [{ id: 'pinned', title: 'Pinned' }],
	render: () => text('root', 'Built against noura:plugin@0.1.0.'),
	sidebar: () => text('side', 'Pinned'),
	rows: () => [],
	handleEvent(_page: string, event: Event) {
		if (event.tag === 'pressed') {
			openPage('pinned');
		}
	},
};

export const objectTypes: ObjectTypesExports = {
	types: () => [{ name: 'pin', schema: '{"properties":[]}' }],
	renderPanel: (object: string) => text('panel', get(object).title),
	handlePanelEvent() {},
};

export const commands: CommandsExports = {
	commands: () => [
		{ id: 'check', title: 'Pinned: check', palette: true, plugins: true },
	],
	run: () => check(),
};

export const inbox: InboxExports = {
	sections: () => [{ id: 'pinned', title: 'Pinned' }],
	renderSection: () => text('section', 'Pinned to 0.1.0.'),
	handleSectionEvent() {},
};

export const settings: SettingsExports = {
	page: () => 'Pinned',
	renderSettings: () => text('settings', 'Nothing to set.'),
	handleSettingsEvent() {},
};

export const tools: ToolsExports = {
	tools: () => [
		{
			name: 'check',
			description: 'Calls the 0.1.0 imports.',
			inputSchema: '{"type":"object"}',
		},
	],
	callTool: () => check(),
};
