import { describe, expect, test } from 'bun:test';
import { DebouncedSearch, type DebouncedSearchState } from './debounced-search';

class FakeClock {
	now = 0;
	private nextId = 0;
	private tasks = new Map<number, { at: number; callback: () => void }>();

	schedule = (callback: () => void, delayMs: number) => {
		const id = this.nextId++;
		this.tasks.set(id, { at: this.now + delayMs, callback });
		return () => this.tasks.delete(id);
	};

	async advance(milliseconds: number) {
		const destination = this.now + milliseconds;
		while (true) {
			const next = [...this.tasks.entries()]
				.filter(([, task]) => task.at <= destination)
				.sort((left, right) => left[1].at - right[1].at)[0];
			if (!next) break;
			this.now = next[1].at;
			this.tasks.delete(next[0]);
			next[1].callback();
			await Promise.resolve();
			await Promise.resolve();
		}
		this.now = destination;
	}
}

function deferred<T>() {
	let resolve!: (value: T | PromiseLike<T>) => void;
	let reject!: (reason?: unknown) => void;
	const promise = new Promise<T>((success, failure) => {
		resolve = success;
		reject = failure;
	});
	return { promise, resolve, reject };
}

function harness(search: (query: string) => Promise<string[]>) {
	const clock = new FakeClock();
	let state: DebouncedSearchState<string> = {
		results: [],
		searching: false,
		error: null,
	};
	const coordinator = new DebouncedSearch<string>({
		delayMs: 150,
		search,
		schedule: clock.schedule,
		onChange: (next) => (state = next),
	});
	return { clock, coordinator, state: () => state };
}

describe('DebouncedSearch', () => {
	test('clears results as soon as a new query is scheduled', async () => {
		const { clock, coordinator, state } = harness(async (query) => [query]);
		coordinator.update('alpha');
		await clock.advance(150);
		expect(state().results).toEqual(['alpha']);

		coordinator.update('beta');
		expect(state()).toEqual({ results: [], searching: true, error: null });
	});

	test('ignores an older response that finishes after the latest query', async () => {
		const alpha = deferred<string[]>();
		const beta = deferred<string[]>();
		const { clock, coordinator, state } = harness((query) =>
			query === 'alpha' ? alpha.promise : beta.promise,
		);

		coordinator.update('alpha');
		await clock.advance(150);
		coordinator.update('beta');
		await clock.advance(150);
		beta.resolve(['new']);
		await Promise.resolve();
		expect(state().results).toEqual(['new']);

		alpha.resolve(['stale']);
		await Promise.resolve();
		expect(state().results).toEqual(['new']);
	});

	test('reset cancels pending timers and invalidates in-flight searches', async () => {
		const pending = deferred<string[]>();
		let calls = 0;
		const { clock, coordinator, state } = harness(() => {
			calls += 1;
			return pending.promise;
		});

		coordinator.update('before-timer');
		coordinator.reset();
		await clock.advance(150);
		expect(calls).toBe(0);

		coordinator.update('in-flight');
		await clock.advance(150);
		expect(calls).toBe(1);
		coordinator.reset();
		pending.resolve(['late']);
		await Promise.resolve();
		expect(state()).toEqual({ results: [], searching: false, error: null });
	});

	test('shows errors only for the current query', async () => {
		const { clock, coordinator, state } = harness(async () => {
			throw new Error('offline');
		});
		coordinator.update('alpha');
		await clock.advance(150);
		expect(state()).toEqual({
			results: [],
			searching: false,
			error: 'Search is unavailable. Please try again.',
		});
	});
});
