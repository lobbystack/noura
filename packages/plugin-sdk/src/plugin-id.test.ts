import { expect, test } from 'bun:test';
import fixtures from '../../../docs/workspace-format/fixtures/conformance-v1.json';
import { pluginManifestSchema } from './index';

const pluginId = pluginManifestSchema.shape.id;

// Plugin IDs end up in `enabled_plugins`, so the SDK accepts exactly the IDs
// the workspace manifest accepts.
for (const fixture of fixtures.manifest) {
	const ids = fixture.value.enabled_plugins;
	if (!Array.isArray(ids) || ids.length === 0) continue;
	const decidesIds = fixture.name.includes('plugin id');
	if (!fixture.valid && !decidesIds) continue;
	test(`plugin IDs: ${fixture.name}`, () => {
		const accepted = ids.every((id) => pluginId.safeParse(id).success);
		expect(accepted).toBe(fixture.valid);
	});
}
