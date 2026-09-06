import type { CoreEvent } from '@noura/workspace';

export const LIVE_REFRESH_EVENT_TYPES = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
	'file:changed',
	'search:index-updated',
	'workspace:ready',
	'workspace:manifest-updated',
]);

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
			if (isLiveRefreshEvent(event, this.#workspaceId())) {
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

	#recover = () => {
		if (this.#disposed) return;
		this.#ensureSubscribed();
		void this.#coordinator.refreshNow().catch(() => {});
	};

	#recoverWhenVisible = () => {
		if (this.#visibilitySource?.visibilityState === 'visible') this.#recover();
	};
}

export function isLiveRefreshEvent(
	event: CoreEvent,
	workspaceId: string | null | undefined,
): boolean {
	return (
		LIVE_REFRESH_EVENT_TYPES.has(event.type) &&
		(workspaceId === null ||
			workspaceId === undefined ||
			event.workspaceId === workspaceId)
	);
}
