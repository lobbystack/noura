import { expect, test } from 'bun:test';
import { readBlobRange } from './blobs';

function stream(chunks: Uint8Array[]) {
	return new ReadableStream<Uint8Array>({
		start(controller) {
			for (const chunk of chunks) controller.enqueue(chunk);
			controller.close();
		},
	});
}

test('storage ranges require exactly the requested ciphertext bytes', async () => {
	expect(
		await readBlobRange(
			stream([new Uint8Array([1]), new Uint8Array([2, 3])]),
			3,
		),
	).toEqual(new Uint8Array([1, 2, 3]));
	await expect(
		readBlobRange(stream([new Uint8Array(2)]), 3),
	).rejects.toMatchObject({
		code: 'sync.blob_storage_unavailable',
	});
	await expect(
		readBlobRange(stream([new Uint8Array(4)]), 3),
	).rejects.toMatchObject({
		code: 'sync.blob_storage_unavailable',
	});
});

test('a stalled storage read is cancelled before returning any ciphertext', async () => {
	let cancelled = false;
	const pending = new ReadableStream<Uint8Array>({
		cancel() {
			cancelled = true;
		},
	});
	await expect(readBlobRange(pending, 3, 10)).rejects.toMatchObject({
		code: 'sync.blob_storage_unavailable',
	});
	expect(cancelled).toBe(true);
});
