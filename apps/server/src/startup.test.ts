import { expect, test } from 'bun:test';
import { retryReady } from './startup';

test('retries a waking database until the probe succeeds', async () => {
	const delays: number[] = [];
	let attempts = 0;
	await retryReady(
		async () => {
			attempts += 1;
			if (attempts < 3) throw new Error('connect ECONNREFUSED');
		},
		{
			attempts: 5,
			baseDelayMs: 100,
			maxDelayMs: 400,
			sleep: async (ms) => {
				delays.push(ms);
			},
		},
	);
	expect(attempts).toBe(3);
	expect(delays).toEqual([100, 200]);
});

test('gives up after the attempt budget and surfaces the last error', async () => {
	let attempts = 0;
	await expect(
		retryReady(
			async () => {
				attempts += 1;
				throw new Error(`failure ${attempts}`);
			},
			{
				attempts: 4,
				baseDelayMs: 10,
				maxDelayMs: 20,
				sleep: async () => {},
			},
		),
	).rejects.toThrow('failure 4');
	expect(attempts).toBe(4);
});

test('caps the delay and reports each retry', async () => {
	const delays: number[] = [];
	const retries: Array<[number, number]> = [];
	await expect(
		retryReady(
			async () => {
				throw new Error('down');
			},
			{
				attempts: 4,
				baseDelayMs: 100,
				maxDelayMs: 250,
				sleep: async (ms) => {
					delays.push(ms);
				},
				onRetry: (_error, attempt, delayMs) => {
					retries.push([attempt, delayMs]);
				},
			},
		),
	).rejects.toThrow('down');
	expect(delays).toEqual([100, 200, 250]);
	expect(retries).toEqual([
		[1, 100],
		[2, 200],
		[3, 250],
	]);
});

test('a single attempt does not retry', async () => {
	let attempts = 0;
	let slept = 0;
	await expect(
		retryReady(
			async () => {
				attempts += 1;
				throw new Error('once');
			},
			{
				attempts: 1,
				sleep: async () => {
					slept += 1;
				},
			},
		),
	).rejects.toThrow('once');
	expect(attempts).toBe(1);
	expect(slept).toBe(0);
});
