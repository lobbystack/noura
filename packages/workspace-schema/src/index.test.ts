import { describe, expect, test } from 'bun:test';
import { objectIdSchema, workspaceManifestSchema } from './index';

type Fixture = { name: string; valid: boolean; value: unknown };
const fixtures = (await Bun.file(
	new URL(
		'../../../docs/workspace-format/fixtures/conformance-v1.json',
		import.meta.url,
	),
).json()) as { manifest: Fixture[]; object_id: Fixture[] };

describe('workspace format conformance', () => {
	for (const fixture of fixtures.manifest) {
		test(`manifest: ${fixture.name}`, () => {
			expect(workspaceManifestSchema.safeParse(fixture.value).success).toBe(
				fixture.valid,
			);
		});
	}
	for (const fixture of fixtures.object_id) {
		test(`object ID: ${fixture.name}`, () => {
			expect(objectIdSchema.safeParse(fixture.value).success).toBe(
				fixture.valid,
			);
		});
	}
});
