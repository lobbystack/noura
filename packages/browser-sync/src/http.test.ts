import { describe, expect, test } from 'bun:test';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { readBoundedBytes, readJson } from './http';

describe('bounded response reads', () => {
	test('reads a small JSON body', async () => {
		const response = new Response(JSON.stringify({ ok: true }));
		expect(await readJson(response)).toEqual({ ok: true });
	});

	test('rejects a JSON body over the cap and maps it to a structured error', async () => {
		const response = new Response(
			JSON.stringify({ padding: 'x'.repeat(4096) }),
		);
		let error: unknown;
		try {
			await readJson(response, 64);
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.InvalidResponse,
		);
	});

	test('rejects a chunked body over the cap while reading', async () => {
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(new Uint8Array(128));
				controller.enqueue(new Uint8Array(128));
				controller.close();
			},
		});
		const response = new Response(stream);
		await expect(readBoundedBytes(response, 64)).rejects.toBeInstanceOf(
			BrowserSyncError,
		);
	});

	test('accepts a body exactly at the cap', async () => {
		const response = new Response(new Uint8Array(64).fill(7));
		const bytes = await readBoundedBytes(response, 64);
		expect(bytes.length).toBe(64);
	});
});
