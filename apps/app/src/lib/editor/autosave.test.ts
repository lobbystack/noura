import { describe, expect, test } from 'bun:test';
import { AutosaveCoordinator } from './autosave';

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

describe('AutosaveCoordinator', () => {
	test('saves 300 ms after typing stops', async () => {
		const clock = new FakeClock();
		const writes: string[] = [];
		const coordinator = new AutosaveCoordinator<string>({
			now: () => clock.now,
			schedule: clock.schedule,
			write: async (body) => void writes.push(body),
		});

		coordinator.noteEdit('draft');
		await clock.advance(299);
		expect(writes).toEqual([]);
		await clock.advance(1);
		expect(writes).toEqual(['draft']);
	});

	test('saves at least every two seconds during continuous typing', async () => {
		const clock = new FakeClock();
		const writes: string[] = [];
		const coordinator = new AutosaveCoordinator<string>({
			now: () => clock.now,
			schedule: clock.schedule,
			write: async (body) => void writes.push(body),
		});

		coordinator.noteEdit('0');
		for (let elapsed = 250; elapsed < 2000; elapsed += 250) {
			await clock.advance(250);
			coordinator.noteEdit(String(elapsed));
		}
		await clock.advance(250);
		expect(writes).toEqual(['1750']);
	});

	test('coalesces edits and cancels both timers after a durable write', async () => {
		const clock = new FakeClock();
		const writes: string[] = [];
		const coordinator = new AutosaveCoordinator<string>({
			now: () => clock.now,
			schedule: clock.schedule,
			write: async (body) => void writes.push(body),
		});

		coordinator.noteEdit('one');
		await clock.advance(200);
		coordinator.noteEdit('two');
		await clock.advance(300);
		await clock.advance(3000);
		expect(writes).toEqual(['two']);
	});

	test('serializes writes and immediately drains edits made in flight', async () => {
		const first = deferred<void>();
		const writes: string[] = [];
		const coordinator = new AutosaveCoordinator<string>({
			write: async (body) => {
				writes.push(body);
				if (writes.length === 1) await first.promise;
			},
		});

		coordinator.noteEdit('first');
		const flushing = coordinator.flush();
		await Promise.resolve();
		coordinator.noteEdit('second');
		first.resolve();
		await expect(flushing).resolves.toBe(true);
		expect(writes).toEqual(['first', 'second']);
	});

	test('keeps the latest generation when an older response completes', async () => {
		const first = deferred<void>();
		const generations: number[] = [];
		const coordinator = new AutosaveCoordinator<string>({
			write: async (_body, generation) => {
				generations.push(generation);
				if (generations.length === 1) await first.promise;
			},
		});

		coordinator.noteEdit('first');
		const flushing = coordinator.flush();
		await Promise.resolve();
		coordinator.noteEdit('newer');
		first.resolve();
		await flushing;
		expect(generations).toEqual([1, 2]);
		expect(coordinator.currentGeneration).toBe(2);
	});

	test('preserves a failed draft and retries only when asked', async () => {
		let attempts = 0;
		const coordinator = new AutosaveCoordinator<string>({
			write: async () => {
				attempts += 1;
				if (attempts === 1) throw new Error('disk full');
			},
		});

		coordinator.noteEdit('safe draft');
		await expect(coordinator.flush()).resolves.toBe(false);
		expect(coordinator.getDraft()).toBe('safe draft');
		expect(coordinator.error).toBeInstanceOf(Error);
		await expect(coordinator.flush()).resolves.toBe(true);
		expect(attempts).toBe(2);
		expect(coordinator.getDraft()).toBeNull();
	});

	test('pauses conflict writes and resumes the preserved draft', async () => {
		const writes: string[] = [];
		let conflict = true;
		const coordinator = new AutosaveCoordinator<string>({
			write: async (body) => {
				writes.push(body);
				if (conflict) return 'paused';
			},
		});

		coordinator.noteEdit('draft');
		await expect(coordinator.flush()).resolves.toBe(false);
		expect(coordinator.getDraft()).toBe('draft');
		conflict = false;
		coordinator.resume();
		await expect(coordinator.flush()).resolves.toBe(true);
		expect(writes).toEqual(['draft', 'draft']);
	});
});
