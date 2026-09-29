import type { CoreEvent } from '@noura/workspace';
import { BULK_OBJECT_EVENT, objectEvents } from './object-events';

/**
 * Events that can change what a projection shows. `search:index-updated`
 * is left out on purpose: the engine emits it next to an object or file
 * event, so listening to both doubled the work behind every save.
 */
const LIVE_REFRESH_EVENT_TYPES = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
	BULK_OBJECT_EVENT,
	'file:changed',
	'workspace:ready',
	'workspace:manifest-updated',
]);

/** Decides whether a core event can change one projection. */
export type LiveEventFilter = (event: CoreEvent) => boolean;

function payloadObjectType(payload: unknown): string | null {
	if (!payload || typeof payload !== 'object' || !('type' in payload))
		return null;
	return typeof payload.type === 'string' ? payload.type : null;
}

/**
 * A filter for projections built from typed objects. Object events count
 * only for the listed types, so a note autosave no longer reloads tasks. A
 * bulk `objects:changed` event counts when any of its changes does.
 * External file changes still count: they may arrive batched, without a
 * per-object event.
 */
export function objectTypeEvents(types: readonly string[]): LiveEventFilter {
	const accepted = new Set(types);
	return (event) => {
		const changes = objectEvents(event);
		if (changes.length > 0)
			return changes.some((change) => {
				const type = payloadObjectType(change.payload);
				return type === null || accepted.has(type);
			});
		if (event.type === BULK_OBJECT_EVENT) return true;
		if (event.type === 'file:changed') return event.source !== 'application';
		return (
			event.type === 'workspace:ready' ||
			event.type === 'workspace:manifest-updated'
		);
	};
}

type Schedule = (callback: () => void, delayMs: number) => () => void;

const defaultSchedule: Schedule = (callback, delayMs) => {
	const timer = setTimeout(callback, delayMs);
	return () => clearTimeout(timer);
};

export interface LiveRefreshOptions {
	refresh: () => Promise<void>;
	onError?: (error: unknown) => void;
	delayMs?: number;
	schedule?: Schedule;
}

type EventSource = Pick<
	EventTarget,
	'addEventListener' | 'removeEventListener'
>;
type VisibilitySource = EventSource & { visibilityState: string };

export interface LiveProjectionOptions extends LiveRefreshOptions {
	subscribe: (handler: (event: CoreEvent) => void) => Promise<() => void>;
	workspaceId: () => string | null | undefined;
	/** Which events can change this projection. Defaults to every event in
	 * LIVE_REFRESH_EVENT_TYPES. */
	events?: LiveEventFilter;
	focusSource?: EventSource;
	visibilitySource?: VisibilitySource;
}

/**
 * Serializes projection reads after core events. Core events are hints, so a
 * refresh always reads through the typed client rather than trusting payloads.
 */
export class LiveRefresh {
	readonly #refresh: () => Promise<void>;
	readonly #onError?: (error: unknown) => void;
	readonly #delayMs: number;
	readonly #schedule: Schedule;
	#cancelTimer: (() => void) | undefined;
	#chain: Promise<void> = Promise.resolve();
	#disposed = false;

	constructor(options: LiveRefreshOptions) {
		this.#refresh = options.refresh;
		this.#onError = options.onError;
		this.#delayMs = options.delayMs ?? 250;
		this.#schedule = options.schedule ?? defaultSchedule;
	}

	invalidate(): void {
		if (this.#disposed) return;
		this.#cancelTimer?.();
		this.#cancelTimer = this.#schedule(() => {
			this.#cancelTimer = undefined;
			void this.refreshNow().catch(() => {});
		}, this.#delayMs);
	}

	refreshNow(): Promise<void> {
		if (this.#disposed) return Promise.resolve();
		this.#cancelTimer?.();
		this.#cancelTimer = undefined;
		const run = this.#chain.then(async () => {
			if (!this.#disposed) await this.#refresh();
		});
		this.#chain = run.catch((error: unknown) => {
			if (!this.#disposed) this.#onError?.(error);
		});
		return run;
	}

	dispose(): void {
		this.#disposed = true;
		this.#cancelTimer?.();
		this.#cancelTimer = undefined;
	}
}

/**
 * Owns the complete lifecycle for a live, disposable projection: initial
 * loading, event invalidation, missed-event recovery, and cleanup.
 */
export class LiveProjection {
	readonly #coordinator: LiveRefresh;
	readonly #subscribe: LiveProjectionOptions['subscribe'];
	readonly #workspaceId: LiveProjectionOptions['workspaceId'];
	readonly #events: LiveEventFilter | undefined;
	readonly #focusSource?: EventSource;
	readonly #visibilitySource?: VisibilitySource;
	readonly #onError?: (error: unknown) => void;
	#subscription: (() => void) | undefined;
	#subscriptionPending: Promise<void> | undefined;
	#started = false;
	#disposed = false;

	constructor(options: LiveProjectionOptions) {
		this.#subscribe = options.subscribe;
		this.#workspaceId = options.workspaceId;
		this.#events = options.events;
		this.#focusSource = options.focusSource;
		this.#visibilitySource = options.visibilitySource;
		this.#onError = options.onError;
		this.#coordinator = new LiveRefresh(options);
	}

	start(): Promise<void> {
		if (this.#disposed) return Promise.resolve();
		if (!this.#started) {
			this.#started = true;
			this.#focusSource?.addEventListener('focus', this.#recover);
			this.#visibilitySource?.addEventListener(
				'visibilitychange',
				this.#recoverWhenVisible,
			);
			this.#ensureSubscribed();
		}
		return this.#coordinator.refreshNow();
	}

	invalidate(): void {
		this.#coordinator.invalidate();
	}

	refreshNow(): Promise<void> {
		if (this.#started && !this.#disposed) this.#ensureSubscribed();
		return this.#coordinator.refreshNow();
	}

	dispose(): void {
		if (this.#disposed) return;
		this.#disposed = true;
		this.#coordinator.dispose();
		this.#subscription?.();
		this.#subscription = undefined;
		this.#focusSource?.removeEventListener('focus', this.#recover);
		this.#visibilitySource?.removeEventListener(
			'visibilitychange',
			this.#recoverWhenVisible,
		);
	}

	#ensureSubscribed(): void {
		if (this.#disposed || this.#subscription || this.#subscriptionPending)
			return;
		this.#subscriptionPending = this.#subscribe((event) => {
			if (
				isLiveRefreshEvent(event, this.#workspaceId(), this.#events) &&
				!this.#disposed
			) {
				this.#coordinator.invalidate();
			}
		})
			.then((unsubscribe) => {
				if (this.#disposed) unsubscribe();
				else this.#subscription = unsubscribe;
			})
			.catch((error: unknown) => {
				if (!this.#disposed) this.#onError?.(error);
			})
			.finally(() => {
				this.#subscriptionPending = undefined;
			});
	}

	// Focus and visibilitychange both fire when the window comes back, so
	// recovery goes through the debounced path and runs one read, not two.
	#recover = () => {
		if (this.#disposed) return;
		this.#ensureSubscribed();
		this.#coordinator.invalidate();
	};

	#recoverWhenVisible = () => {
		if (this.#visibilitySource?.visibilityState === 'visible') this.#recover();
	};
}

export function isLiveRefreshEvent(
	event: CoreEvent,
	workspaceId: string | null | undefined,
	events?: LiveEventFilter,
): boolean {
	const relevant = events
		? events(event)
		: LIVE_REFRESH_EVENT_TYPES.has(event.type);
	return (
		relevant &&
		(workspaceId === null ||
			workspaceId === undefined ||
			event.workspaceId === workspaceId)
	);
}

type Subscribe = (handler: (event: CoreEvent) => void) => Promise<() => void>;

/**
 * Shares one host event listener between every subscriber. Each Tauri
 * listener costs an IPC round trip and its own delivery, and the app had
 * six to ten of them. The shared listener stays for the app's lifetime;
 * a failed attach is retried by the next subscriber.
 */
export function shareEventSubscription(subscribe: Subscribe): Subscribe {
	const handlers = new Set<(event: CoreEvent) => void>();
	let attached: Promise<unknown> | null = null;
	const dispatch = (event: CoreEvent) => {
		for (const handler of [...handlers]) {
			try {
				handler(event);
			} catch (error) {
				// One failing subscriber must not starve the others.
				console.error(error);
			}
		}
	};
	return async (handler) => {
		attached ??= subscribe(dispatch).catch((error: unknown) => {
			attached = null;
			throw error;
		});
		await attached;
		// A wrapper keeps each registration distinct, even for a reused handler.
		const registration = (event: CoreEvent) => handler(event);
		handlers.add(registration);
		return () => {
			handlers.delete(registration);
		};
	};
}
