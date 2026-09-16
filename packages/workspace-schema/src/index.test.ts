import { describe, expect, test } from 'bun:test';
import {
	chatFrontmatterSchema,
	chatMessageFrontmatterSchema,
	objectIdSchema,
	projectPropertiesSchema,
	taskPropertiesSchema,
	workspaceManifestSchema,
} from './index';

type Fixture = { name: string; valid: boolean; value: unknown };
const fixtures = (await Bun.file(
	new URL(
		'../../../docs/workspace-format/fixtures/conformance-v1.json',
		import.meta.url,
	),
).json()) as {
	manifest: Fixture[];
	object_id: Fixture[];
	task_properties: Fixture[];
	project_properties: Fixture[];
	chat: Fixture[];
	chat_message: Fixture[];
};

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
	for (const fixture of fixtures.task_properties) {
		test(`task properties: ${fixture.name}`, () => {
			expect(taskPropertiesSchema.safeParse(fixture.value).success).toBe(
				fixture.valid,
			);
		});
	}
	for (const fixture of fixtures.project_properties) {
		test(`project properties: ${fixture.name}`, () => {
			expect(projectPropertiesSchema.safeParse(fixture.value).success).toBe(
				fixture.valid,
			);
		});
	}
	for (const fixture of fixtures.chat) {
		test(`chat: ${fixture.name}`, () => {
			expect(chatFrontmatterSchema.safeParse(fixture.value).success).toBe(
				fixture.valid,
			);
		});
	}
	for (const fixture of fixtures.chat_message) {
		test(`chat message: ${fixture.name}`, () => {
			expect(
				chatMessageFrontmatterSchema.safeParse(fixture.value).success,
			).toBe(fixture.valid);
		});
	}

	test('omitted optional chat message fields normalize to null', () => {
		const fixture = fixtures.chat_message.find(
			({ name }) =>
				name === 'context summary may omit unrelated optional fields',
		);
		const parsed = chatMessageFrontmatterSchema.safeParse(fixture?.value);

		expect(parsed.success).toBe(true);
		if (parsed.success) {
			expect(parsed.data.provider_id).toBeNull();
			expect(parsed.data.model_id).toBeNull();
			expect(parsed.data.tool_call_id).toBeNull();
			expect(parsed.data.tool_name).toBeNull();
			expect(parsed.data.error_code).toBeNull();
		}
	});

	test('chat metadata is not accepted on the wrong message kind', () => {
		const base = structuredClone(
			fixtures.chat_message.find(
				({ name }) => name === 'completed user message with unknown property',
			)?.value,
		) as Record<string, unknown>;

		expect(
			chatMessageFrontmatterSchema.safeParse({
				...base,
				provider_id: 'provider-that-does-not-belong-on-a-user-message',
			}).success,
		).toBe(false);
		expect(
			chatMessageFrontmatterSchema.safeParse({
				...base,
				tool_call_id: 'call-that-does-not-belong-on-a-user-message',
			}).success,
		).toBe(false);
	});
});
