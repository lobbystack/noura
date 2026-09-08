import { expect, test } from 'bun:test';
import { PdfRangeReader } from './range-reader';
import type { PdfRangeInput } from '@noura/workspace';
const info = {
	relativePath: 'large.pdf',
	workspaceId: 'w',
	version: 'v',
	length: 256 * 1024 * 1024,
};
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
test('coalesced requests are split into bounded IPC reads carrying the file version', async () => {
	const calls: PdfRangeInput[] = [];
	const completed: { begin: number; bytes: Uint8Array }[] = [];
	const errors: unknown[] = [];
	const reader = new PdfRangeReader(
		info,
		async (input) => {
			calls.push(input);
			return new Uint8Array(input.length).fill(input.offset / 1024 / 1024);
		},
		(begin, bytes) => completed.push({ begin, bytes }),
		(e) => errors.push(e),
	);
	reader.request(1024 * 1024, 3 * 1024 * 1024 + 7);
	await tick();
	expect(calls.map((c) => c.length)).toEqual([1024 * 1024, 1024 * 1024, 7]);
	expect(
		calls.every(
			(c) =>
				c.workspaceId === 'w' &&
				c.version === 'v' &&
				c.relativePath === 'large.pdf',
		),
	).toBe(true);
	expect(completed.length).toBe(1);
	expect(completed[0].begin).toBe(1024 * 1024);
	expect(completed[0].bytes[0]).toBe(1);
	expect(completed[0].bytes.at(-1)).toBe(3);
	expect(errors).toEqual([]);
});
test('at most two reads run concurrently and abort discards queued and late results', async () => {
	const pending: ((bytes: Uint8Array) => void)[] = [];
	let delivered = 0;
	const reader = new PdfRangeReader(
		info,
		() => new Promise((resolve) => pending.push(resolve)),
		() => delivered++,
		() => {},
	);
	for (let i = 0; i < 10; i++) reader.request(i * 10, i * 10 + 10);
	expect(pending.length).toBe(2);
	reader.abort();
	pending.forEach((resolve) => resolve(new Uint8Array(10)));
	await tick();
	expect(pending.length).toBe(2);
	expect(delivered).toBe(0);
});
test('structured errors stop pending work and reach the viewer unchanged', async () => {
	const error = { code: 'pdf_changed', message: 'Changed', retryable: true };
	let calls = 0;
	const errors: unknown[] = [];
	const reader = new PdfRangeReader(
		info,
		async () => {
			calls++;
			throw error;
		},
		() => {
			throw new Error('Unexpected response');
		},
		(e) => errors.push(e),
	);
	for (let i = 0; i < 10; i++) reader.request(i * 10, i * 10 + 10);
	await tick();
	expect(calls).toBe(2);
	expect(errors).toEqual([error]);
});
test('invalid intervals and short responses fail visibly', async () => {
	for (const [begin, end] of [
		[-1, 2],
		[0, 0],
		[0, info.length + 1],
		[0, NaN],
		[1.5, 10],
	]) {
		const errors: unknown[] = [];
		const reader = new PdfRangeReader(
			info,
			async () => {
				throw new Error('Unexpected read');
			},
			() => {},
			(e) => errors.push(e),
		);
		reader.request(begin, end);
		expect(errors.length).toBe(1);
	}
	const errors: unknown[] = [];
	new PdfRangeReader(
		info,
		async () => new Uint8Array(1),
		() => {},
		(e) => errors.push(e),
	).request(0, 10);
	await tick();
	expect(errors.length).toBe(1);
});
