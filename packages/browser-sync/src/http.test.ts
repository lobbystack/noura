import { describe, expect, test } from 'bun:test';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { createSameOriginFetch, readBoundedBytes, readJson } from './http';

const ORIGIN = 'https://sync.example';

function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' },
	});
}

function requestUrl(input: RequestInfo | URL): string {
	if (typeof input === 'string') return input;
	if (input instanceof URL) return input.toString();
	return input.url;
}

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

describe('same-origin fetch', () => {
	test('same-origin fetch rejects cross-origin requests', async () => {
		const seen: string[] = [];
		const sameOrigin = createSameOriginFetch(ORIGIN, async (input) => {
			seen.push(requestUrl(input));
			return json({ ok: true });
		});
		await sameOrigin('/v1/device-challenges');
		expect(seen).toEqual(['/v1/device-challenges']);
		await expect(
			sameOrigin('https://other.example/v1/device-challenges'),
		).rejects.toThrow('same-origin');
	});

	test('same-origin fetch forces redirect: error', async () => {
		let captured: RequestInit | undefined;
		const sameOrigin = createSameOriginFetch(ORIGIN, async (_input, init) => {
			captured = init;
			return json({ ok: true });
		});
		await sameOrigin('/v1/device-challenges');
		expect(captured?.redirect).toBe('error');
	});
});
