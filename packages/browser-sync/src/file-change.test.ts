import { describe, expect, test } from 'bun:test';
import fixture from '../../../docs/workspace-format/fixtures/sync-v1.json';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	decodeFileChange,
	encodeFileChange,
	type FileChange,
	type FileChangeInput,
} from './index';

interface ValidVector {
	input: Record<string, unknown>;
	canonical: string;
}

interface InvalidVector {
	[key: string]: unknown;
}

interface SyncFixture {
	valid: ValidVector[];
	invalid: InvalidVector[];
}

const syncFixture = fixture as unknown as SyncFixture;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function normalizedEquivalent(input: Record<string, unknown>): FileChange {
	return {
		...input,
		previousPath: input.previousPath ?? null,
		baseRevision: input.baseRevision ?? null,
		content: input.content ?? null,
	} as FileChange;
}

describe('sync-v1 file-change fixture', () => {
	test('declares valid and invalid vectors', () => {
		expect(syncFixture.valid.length).toBeGreaterThan(0);
		expect(syncFixture.invalid.length).toBeGreaterThan(0);
	});

	for (const vector of syncFixture.valid) {
		test(`encodes exact canonical bytes: ${String(vector.input.path)}`, () => {
			const encoded = encodeFileChange(
				vector.input as unknown as FileChangeInput,
			);
			expect(Array.from(encoded)).toEqual(
				Array.from(encoder.encode(vector.canonical)),
			);
			expect(decoder.decode(encoded)).toBe(vector.canonical);
		});

		test(`decodes the canonical bytes: ${String(vector.input.path)}`, () => {
			const decoded = decodeFileChange(encoder.encode(vector.canonical));
			expect(decoded).toEqual(normalizedEquivalent(vector.input));
		});
	}

	for (const vector of syncFixture.invalid) {
		test(`rejects invalid vector: ${JSON.stringify(vector).slice(0, 60)}`, () => {
			let error: unknown;
			try {
				decodeFileChange(encoder.encode(JSON.stringify(vector)));
			} catch (caught) {
				error = caught;
			}
			expect(error).toBeInstanceOf(BrowserSyncError);
			expect((error as BrowserSyncError).code).toBe(
				BrowserSyncErrorCode.InvalidFileChange,
			);
		});
	}
});

describe('decodeFileChange error handling', () => {
	test('rejects bytes that are not valid UTF-8', () => {
		const invalidUtf8 = new Uint8Array([0x7b, 0xff, 0x7d]);
		expect(() => decodeFileChange(invalidUtf8)).toThrow(BrowserSyncError);
		try {
			decodeFileChange(invalidUtf8);
		} catch (error) {
			expect((error as BrowserSyncError).code).toBe(
				BrowserSyncErrorCode.InvalidFileChange,
			);
		}
	});

	test('rejects bytes that are not valid JSON', () => {
		expect(() => decodeFileChange(encoder.encode('not json'))).toThrow(
			BrowserSyncError,
		);
	});
});
