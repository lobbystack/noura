import assert from 'node:assert/strict';
import { WorkspaceFormatError, loadWorkspaceFormat } from '../src/index';

type Fixtures = {
	manifest: Array<{ valid: boolean; value: Record<string, unknown> }>;
	object_id: Array<{ valid: boolean; value: string }>;
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

for (const fixture of fixtures.object_id)
	assert.equal(format.isValidObjectId(fixture.value, 'note'), fixture.valid);

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

console.info('Verified generated wasm glue against workspace format fixtures.');
