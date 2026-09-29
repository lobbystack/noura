import { describe, expect, test } from 'bun:test';
import fixtures from '../../../docs/workspace-format/fixtures/conformance-v1.json';
import { isProjectStatus, isTaskPriority, isTaskStatus } from './enums';

type Fixture = { name: string; valid: boolean; value: Record<string, unknown> };

/**
 * A fixture decides a guard when the property is its only value: then the
 * fixture is valid exactly when the guard accepts it. Any valid fixture also
 * needs every string value of the property to pass the guard.
 */
function checkGuard(
	list: Fixture[],
	property: string,
	guard: (value: unknown) => boolean,
) {
	for (const fixture of list) {
		const value = fixture.value[property];
		if (typeof value !== 'string') continue;
		test(`${property}: ${fixture.name}`, () => {
			if (Object.keys(fixture.value).length === 1)
				expect(guard(value)).toBe(fixture.valid);
			else if (fixture.valid) expect(guard(value)).toBe(true);
		});
	}
}

describe('enum guards follow the shared conformance fixtures', () => {
	checkGuard(fixtures.task_properties as Fixture[], 'status', isTaskStatus);
	checkGuard(fixtures.task_properties as Fixture[], 'priority', isTaskPriority);
	checkGuard(
		fixtures.project_properties as Fixture[],
		'status',
		isProjectStatus,
	);
});
