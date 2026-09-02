export interface DebouncedSearchState<T> {
	results: T[];
	searching: boolean;
	error: string | null;
}

type Schedule = (callback: () => void, delayMs: number) => () => void;

interface DebouncedSearchOptions<T> {
	delayMs: number;
	search: (query: string) => Promise<T[]>;
	onChange: (state: DebouncedSearchState<T>) => void;
	schedule?: Schedule;
}

const defaultSchedule: Schedule = (callback, delayMs) => {
	const timer = setTimeout(callback, delayMs);
	return () => clearTimeout(timer);
};

/** Coordinates a debounced search while ensuring only the latest query wins. */
export class DebouncedSearch<T> {
	readonly #delayMs: number;
	readonly #search: (query: string) => Promise<T[]>;
	readonly #onChange: (state: DebouncedSearchState<T>) => void;
	readonly #schedule: Schedule;
	#generation = 0;
	#cancelTimer: (() => void) | undefined;
	#disposed = false;

	constructor(options: DebouncedSearchOptions<T>) {
		this.#delayMs = options.delayMs;
		this.#search = options.search;
		this.#onChange = options.onChange;
		this.#schedule = options.schedule ?? defaultSchedule;
	}

	update(query: string): void {
		const generation = ++this.#generation;
		this.#cancelPendingTimer();
		if (this.#disposed) return;
		if (query.trim().length === 0) {
			this.#emit({ results: [], searching: false, error: null });
			return;
		}

		// Never present results from the previous query under the new input.
		this.#emit({ results: [], searching: true, error: null });
		this.#cancelTimer = this.#schedule(() => {
			this.#cancelTimer = undefined;
			void this.#run(query, generation);
		}, this.#delayMs);
	}

	reset(): void {
		this.#generation += 1;
		this.#cancelPendingTimer();
		if (!this.#disposed) {
			this.#emit({ results: [], searching: false, error: null });
		}
	}

	dispose(): void {
		this.#disposed = true;
		this.#generation += 1;
		this.#cancelPendingTimer();
	}

	async #run(query: string, generation: number): Promise<void> {
		try {
			const results = await this.#search(query);
			if (this.#isCurrent(generation)) {
				this.#emit({ results, searching: false, error: null });
			}
		} catch {
			if (this.#isCurrent(generation)) {
				this.#emit({
					results: [],
					searching: false,
					error: 'Search is unavailable. Please try again.',
				});
			}
		}
	}

	#isCurrent(generation: number): boolean {
		return !this.#disposed && generation === this.#generation;
	}

	#cancelPendingTimer(): void {
		this.#cancelTimer?.();
		this.#cancelTimer = undefined;
	}

	#emit(state: DebouncedSearchState<T>): void {
		this.#onChange(state);
	}
}
