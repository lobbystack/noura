import { expect, test } from 'bun:test';
import fixtures from '../../../docs/workspace-format/fixtures/sync-v1.json';
import { syncFileChangeSchema } from './sync';

test('sync changes accept exactly the shared Rust conformance fixtures', () => {
	for (const { input, canonical } of fixtures.valid) {
		expect(syncFileChangeSchema.parse(input)).toEqual(JSON.parse(canonical));
	}
	for (const input of fixtures.invalid)
		expect(syncFileChangeSchema.safeParse(input).success).toBe(false);
});
