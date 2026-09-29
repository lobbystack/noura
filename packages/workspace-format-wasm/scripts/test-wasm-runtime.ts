import assert from 'node:assert/strict';
import { WorkspaceFormatError, loadWorkspaceFormat } from '../src/index';

type Fixtures = {
	manifest: Array<{ valid: boolean; value: Record<string, unknown> }>;
	object_id: Array<{ valid: boolean; value: string }>;
	task_properties: Array<{ valid: boolean; value: Record<string, unknown> }>;
	project_properties: Array<{
		valid: boolean;
		value: Record<string, unknown>;
	}>;
};

const wasmModule = new URL('../wasm/workspace_format_wasm.js', import.meta.url);
const wasmBinary = new URL(
	'../wasm/workspace_format_wasm_bg.wasm',
	import.meta.url,
);
const fixtureFile = new URL(
	'../../../docs/workspace-format/fixtures/conformance-v1.json',
	import.meta.url,
);
const fixtures = (await Bun.file(fixtureFile).json()) as Fixtures;
const format = await loadWorkspaceFormat(
	wasmModule.href,
	await Bun.file(wasmBinary).arrayBuffer(),
);
const encoder = new TextEncoder();

for (const fixture of fixtures.manifest) {
	const bytes = encoder.encode(JSON.stringify(fixture.value));
	if (!fixture.valid) {
		assert.throws(
			() => format.parseWorkspaceManifest(bytes),
			(error: unknown) => error instanceof WorkspaceFormatError,
		);
		continue;
	}
	const expected = structuredClone(fixture.value) as {
		enabled_plugins: string[];
	};
	expected.enabled_plugins.sort();
	expected.enabled_plugins = [...new Set(expected.enabled_plugins)];
	assert.deepEqual(format.parseWorkspaceManifest(bytes), expected);
}

const manifest = format.createWorkspaceManifest(
	'Plugin preferences',
	'2026-09-12T00:00:00Z',
);
const updatedManifest = format.updateWorkspaceManifest(
	manifest,
	{ enabledPlugins: ['tasks', 'notes', 'tasks'] },
	'2026-09-12T00:00:01Z',
);
assert.deepEqual(updatedManifest.enabled_plugins, ['notes', 'tasks']);
assert.equal(updatedManifest.updated, '2026-09-12T00:00:01Z');
assert.throws(
	() =>
		format.updateWorkspaceManifest(
			manifest,
			{ enabledPlugins: ['Not valid'] },
			'2026-09-12T00:00:01Z',
		),
	(error: unknown) =>
		error instanceof WorkspaceFormatError && error.code === 'invalid_plugin_id',
);

for (const fixture of fixtures.object_id)
	assert.equal(format.isValidObjectId(fixture.value, 'note'), fixture.valid);

for (const fixture of fixtures.task_properties) {
	const create = () =>
		format.createTask({
			title: 'Fixture task',
			properties: fixture.value,
			now: '2026-09-12T00:00:00Z',
		});
	if (fixture.valid) {
		const task = create();
		assert.equal(task.type, 'task');
		continue;
	}
	assert.throws(
		create,
		(error: unknown) => error instanceof WorkspaceFormatError,
	);
}

for (const fixture of fixtures.project_properties) {
	const create = () =>
		format.createProject({
			title: 'Fixture project',
			properties: fixture.value,
			now: '2026-09-12T00:00:00Z',
		});
	if (fixture.valid) {
		const project = create();
		assert.equal(project.type, 'project');
		continue;
	}
	assert.throws(
		create,
		(error: unknown) => error instanceof WorkspaceFormatError,
	);
}

const object = {
	id: 'note_01j00000000000000000000000',
	type: 'note',
	title: 'Wasm boundary',
	body: 'Canonical bytes',
	relativePath: 'notes/wasm.md',
	revision: '',
	created: '2026-08-27T12:00:00Z',
	updated: '2026-08-27T12:00:00Z',
	properties: { custom: 'kept' },
};
const bytes = format.serializeObject(object);
const parsed = format.parseMarkdown(object.relativePath, bytes);
assert.equal(parsed.kind, 'managed');
assert.deepEqual(format.serializeObject(parsed), bytes);

const created = format.createNote({
	title: 'Created in Rust',
	body: 'The worker persists these canonical bytes.',
	now: '2026-09-12T00:00:00Z',
});
assert.equal(created.type, 'note');
assert.equal(format.isValidObjectId(created.id, 'note'), true);
const createdBytes = format.serializeObject(created);
const createdParsed = format.parseMarkdown(created.relativePath, createdBytes);
assert.equal(createdParsed.kind, 'managed');
assert.deepEqual(format.serializeObject(createdParsed), createdBytes);

const historical = format.createNote({
	title: 'Imported history',
	now: '1960-01-01T00:00:00Z',
});
assert.equal(historical.created, '1960-01-01T00:00:00Z');

const task = format.createTask({
	title: 'Patch metadata',
	properties: { custom: 'kept', priority: 'high' },
	now: '2026-09-12T00:00:00Z',
});
const patchedTask = format.updateTask(task, {
	properties: { status: 'done' },
	removeProperties: ['custom', 'priority'],
	now: '2026-09-12T01:00:00Z',
});
assert.deepEqual(patchedTask.properties, {
	status: 'done',
	priority: 'medium',
});

const project = format.createProject({
	title: 'Canonical project',
	properties: { custom: 'kept' },
	now: '2026-09-12T00:00:00Z',
});
assert.equal(project.relativePath.endsWith('/project.md'), true);
const patchedProject = format.updateProject(project, {
	properties: { status: 'active' },
	now: '2026-09-12T01:00:00Z',
});
assert.deepEqual(patchedProject.properties, {
	status: 'active',
	custom: 'kept',
});

assert.equal(
	format.mergeText(
		'one\n\nmiddle\n\ntwo\n',
		'one local\n\nmiddle\n\ntwo\n',
		'one\n\nmiddle\n\ntwo external\n',
	),
	'one local\n\nmiddle\n\ntwo external\n',
);
assert.equal(format.mergeText('same\n', 'local\n', 'external\n'), null);
assert.equal(
	format.mergeMarkdownBody(
		'<div>\n\nbody\n',
		'<div>\n\nbody local\n',
		'<div>\n\nbody\n',
	),
	null,
);
assert.equal(
	format.mergeMarkdownBody('a\n\nb\n', 'a local\n\nb\n', 'a\n\nb external\n'),
	'a local\n\nb external\n',
);

const canonicalTask = {
	...patchedTask,
	title: 'External title',
	body: 'one\n\ntwo external\n',
	properties: { status: 'todo', priority: 'high' },
};
const mergedTask = format.mergeManagedDraft(
	{
		baseTitle: 'Patch metadata',
		localTitle: 'Patch metadata',
		baseBody: 'one\n\ntwo\n',
		localBody: 'one local\n\ntwo\n',
		baseProperties: { status: 'todo', priority: 'medium' },
		localProperties: { status: 'done', priority: 'medium' },
	},
	canonicalTask,
);
assert.ok(mergedTask);
assert.equal(mergedTask.title, 'External title');
assert.equal(mergedTask.body, 'one local\n\ntwo external\n');
assert.deepEqual(mergedTask.properties, { status: 'done', priority: 'high' });
assert.equal(mergedTask.id, canonicalTask.id);
assert.equal(
	format.mergeManagedDraft(
		{
			baseTitle: 'Patch metadata',
			localTitle: 'Patch metadata',
			baseBody: 'one\n\ntwo\n',
			localBody: 'one\n\ntwo\n',
			baseProperties: { status: 'todo' },
			localProperties: { status: 'done' },
		},
		{ ...canonicalTask, properties: { status: 'in-progress' } },
	),
	null,
);

const crlf = encoder.encode('﻿one\r\ntwo\nthree\r\nfour\r\n');
const raw = format.readRawText(crlf);
assert.deepEqual(raw, {
	body: 'one\ntwo\nthree\nfour\n',
	usesCrlf: true,
	hasBom: true,
});
assert.deepEqual(format.composeRawText(crlf, raw.body), crlf);
assert.deepEqual(
	format.composeRawText(crlf, raw.body.replace('three', 'THREE')),
	encoder.encode('﻿one\r\ntwo\nTHREE\r\nfour\r\n'),
);
assert.throws(
	() => format.readRawText(new Uint8Array([0xff, 0xfe])),
	(error: unknown) =>
		error instanceof WorkspaceFormatError && error.code === 'invalid_utf8',
);
assert.match(format.rawHistorySegment('notes/raw.md'), /^raw-[0-9a-f]{16}$/);
assert.notEqual(
	format.rawHistorySegment('notes/raw.md'),
	format.rawHistorySegment('notes/other.md'),
);

const calendarEntry = (
	start: string,
	title: string,
	sourceId: string,
	property = 'due',
) => ({
	sourceId,
	sourceType: 'task',
	title,
	property,
	start,
	end: null,
	allDay: start.length === 10,
	revision: 'revision',
});
const selected = format.selectCalendarEntries(
	[
		calendarEntry('2026-09-04T12:00:00Z', 'Beta', 'task_c'),
		calendarEntry('2026-09-04T12:00:00Z', 'Alpha', 'task_b', 'start'),
		calendarEntry('2026-09-04T12:00:00Z', 'Alpha', 'task_a'),
		calendarEntry('2026-09-04T13:30:00+02:00', 'Earlier', 'task_d'),
		calendarEntry('2026-09-04', 'All day', 'task_e', 'date'),
		calendarEntry('2026-09-05T00:00:00Z', 'Outside', 'task_f'),
		calendarEntry('not-a-date', 'Malformed', 'task_g'),
	],
	'2026-09-04T00:00:00Z',
	'2026-09-05T00:00:00Z',
);
assert.deepEqual(
	selected.map((entry) => [entry.title, entry.sourceId, entry.property]),
	[
		['All day', 'task_e', 'date'],
		['Earlier', 'task_d', 'due'],
		['Alpha', 'task_a', 'due'],
		['Alpha', 'task_b', 'start'],
		['Beta', 'task_c', 'due'],
	],
);
assert.deepEqual(
	selected[0],
	calendarEntry('2026-09-04', 'All day', 'task_e', 'date'),
);
assert.throws(
	() => format.selectCalendarEntries([], 'soon', '2026-09-05'),
	(error: unknown) =>
		error instanceof WorkspaceFormatError &&
		error.code === 'invalid_calendar_date',
);
assert.throws(
	() => format.selectCalendarEntries([], '2026-09-05', '2026-09-04'),
	(error: unknown) =>
		error instanceof WorkspaceFormatError &&
		error.code === 'invalid_calendar_range',
);

console.info('Verified generated wasm glue against workspace format fixtures.');
