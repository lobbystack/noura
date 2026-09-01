export interface AutosaveState {
	isWriting: boolean;
	pendingEdits: number;
	error: unknown | null;
}

export type AutosaveWriteResult = void | 'paused';

export interface AutosaveOptions {
	debounceDelayMs?: number;
	maxDelayMs?: number;
	now?: () => number;
	schedule?: (callback: () => void, delayMs: number) => () => void;
	/** Resolves only after the submitted generation is durable. */
	write: (body: string, generation: number) => Promise<AutosaveWriteResult>;
	onStateChange?: (state: AutosaveState) => void;
}

const DEFAULT_DEBOUNCE_MS = 300;
const DEFAULT_MAX_DELAY_MS = 2000;

export class AutosaveCoordinator {
	readonly debounceDelayMs: number;
	readonly maxDelayMs: number;
	private readonly writeDraft: AutosaveOptions['write'];
	private readonly schedule: NonNullable<AutosaveOptions['schedule']>;
	private readonly now: NonNullable<AutosaveOptions['now']>;
	private readonly onStateChange?: AutosaveOptions['onStateChange'];

	private pendingBody: string | null = null;
	private pendingCount = 0;
	private generation = 0;
	private dirtySince: number | null = null;
	private cancelDebounce: (() => void) | null = null;
	private cancelMaximum: (() => void) | null = null;
	private running: Promise<boolean> | null = null;
	private paused = false;
	private destroyed = false;
	private failure: unknown | null = null;

	constructor(options: AutosaveOptions) {
		this.writeDraft = options.write;
		this.schedule = options.schedule ?? defaultSchedule;
		this.now = options.now ?? (() => Date.now());
		this.debounceDelayMs = options.debounceDelayMs ?? DEFAULT_DEBOUNCE_MS;
		this.maxDelayMs = options.maxDelayMs ?? DEFAULT_MAX_DELAY_MS;
		this.onStateChange = options.onStateChange;
		if (this.debounceDelayMs < 0) {
			throw new Error('debounceDelayMs must be greater than or equal to 0');
		}
		if (this.maxDelayMs < this.debounceDelayMs) {
			throw new Error(
				'maxDelayMs must be greater than or equal to debounceDelayMs',
			);
		}
	}

	get isWriting() {
		return this.running !== null;
	}

	get pendingEdits() {
		return this.pendingCount;
	}

	get error() {
		return this.failure;
	}

	get currentGeneration() {
		return this.generation;
	}

	get state(): AutosaveState {
		return {
			isWriting: this.isWriting,
			pendingEdits: this.pendingCount,
			error: this.failure,
		};
	}

	noteEdit(body: string) {
		if (this.destroyed) return;
		this.pendingBody = body;
		this.pendingCount += 1;
		this.generation += 1;
		this.dirtySince ??= this.now();
		if (!this.paused && this.failure === null) this.armTimers();
		this.notify();
	}

	getDraft(): string | null {
		return this.pendingBody;
	}

	pause() {
		if (this.destroyed) return;
		this.paused = true;
		this.cancelTimers();
	}

	resume() {
		if (this.destroyed || !this.paused) return;
		this.paused = false;
		if (this.pendingBody !== null && this.failure === null) this.armTimers();
	}

	/** Mark the current editor body as durable and discard queued transactions. */
	acceptDurable() {
		this.pendingBody = null;
		this.pendingCount = 0;
		this.dirtySince = null;
		this.failure = null;
		this.cancelTimers();
		this.notify();
	}

	/** Flush all known generations. Returns false when paused or a write fails. */
	async flush(): Promise<boolean> {
		if (this.destroyed) return false;
		if (this.paused) return this.pendingBody === null;
		this.cancelTimers();
		this.failure = null;
		while (!this.destroyed && !this.paused) {
			if (this.running) {
				if (!(await this.running)) return false;
			} else if (this.pendingBody !== null) {
				if (!(await this.runWrite())) return false;
			} else {
				return true;
			}
		}
		return this.pendingBody === null;
	}

	destroy() {
		this.destroyed = true;
		this.cancelTimers();
	}

	private notify() {
		this.onStateChange?.(this.state);
	}

	private armTimers() {
		if (this.destroyed || this.paused || this.failure !== null) return;
		this.cancelDebounce?.();
		this.cancelDebounce = this.schedule(() => {
			this.cancelDebounce = null;
			void this.flush();
		}, this.debounceDelayMs);
		if (this.cancelMaximum) return;
		const elapsed = this.now() - (this.dirtySince ?? this.now());
		this.cancelMaximum = this.schedule(
			() => {
				this.cancelMaximum = null;
				void this.flush();
			},
			Math.max(0, this.maxDelayMs - elapsed),
		);
	}

	private cancelTimers() {
		this.cancelDebounce?.();
		this.cancelMaximum?.();
		this.cancelDebounce = null;
		this.cancelMaximum = null;
	}

	private async runWrite(): Promise<boolean> {
		if (this.running) return this.running;
		if (this.pendingBody === null) return true;
		const submittedBody = this.pendingBody;
		const submittedGeneration = this.generation;
		this.pendingBody = null;
		this.pendingCount = 0;
		this.dirtySince = null;
		this.cancelTimers();

		const operation = (async () => {
			try {
				const result = await this.writeDraft(
					submittedBody,
					submittedGeneration,
				);
				if (result === 'paused') {
					this.paused = true;
					if (this.pendingBody === null) {
						this.pendingBody = submittedBody;
						this.pendingCount = 1;
						this.dirtySince = this.now();
					}
					return false;
				}
				this.failure = null;
				return true;
			} catch (error) {
				this.failure = error;
				if (this.pendingBody === null) {
					this.pendingBody = submittedBody;
					this.pendingCount = 1;
					this.dirtySince = this.now();
				}
				return false;
			} finally {
				this.running = null;
				this.notify();
			}
		})();
		this.running = operation;
		this.notify();
		return operation;
	}
}

function defaultSchedule(callback: () => void, delayMs: number): () => void {
	const id = setTimeout(callback, delayMs);
	return () => clearTimeout(id);
}
