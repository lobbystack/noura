import { describe, expect, test } from 'bun:test';
import type { CoreEvent } from '@noura/workspace';
import {
	LiveProjection,
	LiveRefresh,
	isLiveRefreshEvent,
} from './live-refresh';

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

	test('keeps the newest serialized result after an older read finishes', async () => {
		const first = deferred<string>();
		let calls = 0;
		let adopted = '';
		const refresh = new LiveRefresh({
			refresh: async () => {
				calls += 1;
				adopted = calls === 1 ? await first.promise : 'newest';
			},
		});

		const older = refresh.refreshNow();
		const newer = refresh.refreshNow();
		first.resolve('older');
		await Promise.all([older, newer]);
		expect(adopted).toBe('newest');
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

function coreEvent(
	type = 'object:updated',
	workspaceId = 'workspace-a',
): CoreEvent {
	return {
		eventId: 'event',
		type,
		workspaceId,
		occurredAt: '2026-09-02T00:00:00Z',
		source: 'external',
		payload: {},
	};
}

class FakeVisibilitySource extends EventTarget {
	visibilityState = 'hidden';
}

describe('LiveProjection', () => {
	test('loads initially and filters event hints by workspace', async () => {
		const clock = new FakeClock();
		let handler: ((event: CoreEvent) => void) | undefined;
		let calls = 0;
		const projection = new LiveProjection({
			refresh: async () => {
				calls += 1;
			},
			subscribe: async (next) => {
				handler = next;
				return () => {};
			},
			workspaceId: () => 'workspace-a',
			schedule: clock.schedule,
		});

		await projection.start();
		handler?.(coreEvent('object:updated', 'workspace-b'));
		await clock.advance(250);
		expect(calls).toBe(1);
		handler?.(coreEvent('workspace:manifest-updated'));
		await clock.advance(250);
		expect(calls).toBe(2);
	});

	test('recovers missed events on focus and visible transitions', async () => {
		const focus = new EventTarget();
		const visibility = new FakeVisibilitySource();
		let calls = 0;
		const projection = new LiveProjection({
			refresh: async () => {
				calls += 1;
			},
			subscribe: async () => () => {},
			workspaceId: () => 'workspace-a',
			focusSource: focus,
			visibilitySource: visibility,
		});

		await projection.start();
		focus.dispatchEvent(new Event('focus'));
		await Promise.resolve();
		await Promise.resolve();
		expect(calls).toBe(2);
		visibility.dispatchEvent(new Event('visibilitychange'));
		await Promise.resolve();
		expect(calls).toBe(2);
		visibility.visibilityState = 'visible';
		visibility.dispatchEvent(new Event('visibilitychange'));
		await Promise.resolve();
		await Promise.resolve();
		expect(calls).toBe(3);
	});

	test('retries a failed subscription with an explicit refresh', async () => {
		let attempts = 0;
		let errors = 0;
		const projection = new LiveProjection({
			refresh: async () => {},
			subscribe: async () => {
				attempts += 1;
				if (attempts === 1) throw new Error('subscription unavailable');
				return () => {};
			},
			workspaceId: () => 'workspace-a',
			onError: () => {
				errors += 1;
			},
		});

		await projection.start();
		await Promise.resolve();
		await Promise.resolve();
		expect(attempts).toBe(1);
		expect(errors).toBe(1);

		await projection.refreshNow();
		await Promise.resolve();
		expect(attempts).toBe(2);
	});

	test('cleans up a subscription that resolves after disposal', async () => {
		const subscription = deferred<() => void>();
		let unsubscribed = false;
		let calls = 0;
		const projection = new LiveProjection({
			refresh: async () => {
				calls += 1;
			},
			subscribe: () => subscription.promise,
			workspaceId: () => 'workspace-a',
		});

		await projection.start();
		projection.dispose();
		subscription.resolve(() => {
			unsubscribed = true;
		});
		await Promise.resolve();
		await Promise.resolve();
		expect(unsubscribed).toBe(true);
		expect(calls).toBe(1);
	});
});
