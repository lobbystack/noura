import { describe, expect, test } from 'bun:test';
import type { Diagnostic } from '@noura/workspace';
import { homeIssues } from './home-issues';

function diagnostic(overrides: Partial<Diagnostic>): Diagnostic {
	return {
		code: 'parse_error',
		message: 'invalid YAML at line 2',
		relativePath: 'notes/a.md',
		objectId: null,
		...overrides,
	};
}

describe('homeIssues', () => {
	test('words known problems without codes or parser output', () => {
		const [issue] = homeIssues([diagnostic({})]);
		expect(issue.message).toBe(
			'noura can’t read the properties at the top of a.md.',
		);
		expect(issue.message).not.toContain('parse_error');
		expect(issue.message).not.toContain('YAML');
		expect(issue.paths).toEqual(['notes/a.md']);
	});

	test('names both files of an identity conflict', () => {
		const [issue] = homeIssues([
			diagnostic({
				code: 'identity_conflict',
				message:
					'Multiple files use this stable ID: notes/a.md, notes/a copy.md',
				relativePath: null,
				objectId: 'note_1',
			}),
		]);
		expect(issue.paths).toEqual(['notes/a.md', 'notes/a copy.md']);
		expect(issue.message).toBe(
			'a.md and a copy.md have the same ID. Remove the id line from the copy.',
		);
	});

	test('keeps distinct problems in one file apart and drops duplicates', () => {
		const first = diagnostic({
			code: 'broken_reference',
			message: 'Referenced object does not exist: task_1',
			objectId: 'note_1',
		});
		const second = {
			...first,
			message: 'Referenced object does not exist: task_2',
		};
		const issues = homeIssues([first, second, first]);
		expect(issues).toHaveLength(2);
		expect(new Set(issues.map((issue) => issue.key)).size).toBe(2);
	});

	test('passes unknown problems through as written', () => {
		const [issue] = homeIssues([
			diagnostic({ code: 'future_problem', message: 'Something new.' }),
		]);
		expect(issue.message).toBe('Something new.');
	});
});
