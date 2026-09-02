import { describe, expect, test } from 'bun:test';
import { LiveRefresh, isLiveRefreshEvent } from './live-refresh';

class FakeClock {
	now = 0;
	#nextId = 0;
	#tasks = new Map<number, { at: number; callback: () => void }>();

	schedule = (callback: () => void, delayMs: number) => {
		const id = this.#nextId++;
		this.#tasks.set(id, { at: this.now + delayMs, callback });
		return () => this.#tasks.delete(id);
	};

	async advance(milliseconds: number) {
		const destination = this.now + milliseconds;
		while (true) {
			const next = [...this.#tasks.entries()]
				.filter(([, task]) => task.at <= destination)
				.sort((left, right) => left[1].at - right[1].at)[0];
			if (!next) break;
			this.now = next[1].at;
			this.#tasks.delete(next[0]);
			next[1].callback();
			await Promise.resolve();
			await Promise.resolve();
			await Promise.resolve();
			await Promise.resolve();
		}
		this.now = destination;
	}
}

function deferred<T>() {
	let resolve!: (value: T | PromiseLike<T>) => void;
	const promise = new Promise<T>((success) => {
		resolve = success;
	});
	return { promise, resolve };
}

describe('LiveRefresh', () => {
	test('coalesces a burst into one trailing refresh', async () => {
		const clock = new FakeClock();
		let calls = 0;
		const refresh = new LiveRefresh({
			refresh: async () => {
				calls += 1;
			},
			schedule: clock.schedule,
		});

		refresh.invalidate();
		await clock.advance(200);
		refresh.invalidate();
		await clock.advance(249);
		expect(calls).toBe(0);
		await clock.advance(1);
		expect(calls).toBe(1);
	});

	test('serializes an explicit refresh behind an in-flight event refresh', async () => {
		const clock = new FakeClock();
		const first = deferred<void>();
		let calls = 0;
		const refresh = new LiveRefresh({
			refresh: () => {
				calls += 1;
				return calls === 1 ? first.promise : Promise.resolve();
			},
			schedule: clock.schedule,
		});

		refresh.invalidate();
		await clock.advance(250);
		expect(calls).toBe(1);
		const second = refresh.refreshNow();
		expect(calls).toBe(1);
		first.resolve();
		await second;
		expect(calls).toBe(2);
	});

	test('reports failures and permits a later retry', async () => {
		const clock = new FakeClock();
		let calls = 0;
		let error: unknown;
		const refresh = new LiveRefresh({
			refresh: async () => {
				calls += 1;
				if (calls === 1) throw new Error('temporary failure');
			},
			onError: (value) => (error = value),
			schedule: clock.schedule,
		});

		refresh.invalidate();
		await clock.advance(250);
		expect(error).toBeInstanceOf(Error);
		await refresh.refreshNow();
		expect(calls).toBe(2);
	});

	test('cancels pending work when disposed', async () => {
		const clock = new FakeClock();
		let calls = 0;
		const refresh = new LiveRefresh({
			refresh: async () => {
				calls += 1;
			},
			schedule: clock.schedule,
		});

		refresh.invalidate();
		refresh.dispose();
		await clock.advance(250);
		expect(calls).toBe(0);
	});
});

describe('isLiveRefreshEvent', () => {
	test('accepts only matching workspace projection events', () => {
		const event = {
			eventId: 'event',
			type: 'object:updated',
			workspaceId: 'workspace-a',
			occurredAt: '2026-09-02T00:00:00Z',
			source: 'external' as const,
			payload: {},
		};
		expect(isLiveRefreshEvent(event, 'workspace-a')).toBe(true);
		expect(isLiveRefreshEvent(event, 'workspace-b')).toBe(false);
		expect(
			isLiveRefreshEvent({ ...event, type: 'workspace:closed' }, 'workspace-a'),
		).toBe(false);
	});
});
