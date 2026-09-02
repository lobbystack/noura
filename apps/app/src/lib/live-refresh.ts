import type { CoreEvent } from '@noura/workspace';

export const LIVE_REFRESH_EVENT_TYPES = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
	'file:changed',
	'search:index-updated',
	'workspace:ready',
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
