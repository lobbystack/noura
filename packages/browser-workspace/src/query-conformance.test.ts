import { describe, expect, test } from 'bun:test';
import type {
	CalendarEntry,
	Diagnostic,
	ObjectSummary,
	WorkspaceEntry,
	WorkspaceObject,
	WorkspaceState,
} from '@noura/shared';
import {
	createServer,
	loadBuiltFormat,
	memoryRegistry,
} from './wasm-test-support';

type Fixture = {
	files: Array<{ path: string; content: string }>;
	objectQueries: Array<{ query: Record<string, unknown>; expected: string[] }>;
	summaryQueries: Array<{ query: Record<string, unknown>; expected: string[] }>;
	calendarQueries: Array<{
		start: string;
		end: string;
		expected?: Array<[string, string]>;
		error?: string;
	}>;
	entries: Array<[string, string, string | null, string | null]>;
	diagnostics: Array<[string, string | null, string | null]>;
	duplicateId: string;
};

const fixture = (await Bun.file(
	new URL(
		'../../../docs/workspace-format/fixtures/queries-v1.json',
		import.meta.url,
	),
).json()) as Fixture;
const format = await loadBuiltFormat();
const encoder = new TextEncoder();

// The same fixture runs against the native index in
// crates/local-core/tests/query_conformance.rs.
describe.skipIf(format === null)('browser queries match native', () => {
	async function workspace() {
		const { registry, directories } = memoryRegistry(format!);
		const { server } = createServer(format!, registry);
		const state = (await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Queries' },
		})) as WorkspaceState;
		const files = directories.get(state.workspaceId!)!;
		for (const file of fixture.files)
			await files.write(file.path, encoder.encode(file.content));
		await server.request('workspace_rebuild_index');
		return server;
	}

	test('object queries', async () => {
		const server = await workspace();
		for (const { query, expected } of fixture.objectQueries) {
			const objects = (await server.request('objects_query', {
				query,
			})) as WorkspaceObject[];
			expect(objects.map((object) => object.id)).toEqual(expected);
		}
	});

	test('summaries', async () => {
		const server = await workspace();
		for (const { query, expected } of fixture.summaryQueries) {
			const summaries = (await server.request('objects_summaries', {
				query,
			})) as ObjectSummary[];
			expect(summaries.map((summary) => summary.id)).toEqual(expected);
		}
	});

	test('calendar', async () => {
		const server = await workspace();
		for (const { start, end, expected, error } of fixture.calendarQueries) {
			const request = server.request('calendar_query', {
				input: { start, end },
			});
			if (error) {
				await expect(request).rejects.toMatchObject({ code: error });
				continue;
			}
			const entries = (await request) as CalendarEntry[];
			expect(entries.map((entry) => [entry.sourceId, entry.property])).toEqual(
				expected!,
			);
		}
	});

	test('file listing', async () => {
		const server = await workspace();
		const entries = (await server.request('files_list')) as WorkspaceEntry[];
		expect(
			entries.map((entry) => [
				entry.relativePath,
				entry.kind,
				entry.parseStatus,
				entry.objectId,
			]),
		).toEqual(fixture.entries);
	});

	test('diagnostics and duplicate IDs', async () => {
		const server = await workspace();
		const state = (await server.request('workspace_state')) as WorkspaceState;
		const diagnostics = state.diagnostics
			.map((value: Diagnostic) => [
				value.code,
				value.relativePath,
				value.objectId,
			])
			.sort((left, right) =>
				JSON.stringify(left) < JSON.stringify(right) ? -1 : 1,
			);
		expect(diagnostics).toEqual(fixture.diagnostics);
		await expect(
			server.request('objects_get', { id: fixture.duplicateId }),
		).rejects.toMatchObject({ code: 'identity_conflict' });
	});
});
