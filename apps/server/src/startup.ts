/**
 * Startup readiness helpers.
 *
 * The sync service may start while the managed database is still waking (for
 * example after scale-to-zero). A single failed probe must not crash the
 * process into a restart loop, so {@link retryReady} waits with bounded
 * exponential backoff and only surfaces the final error.
 */

export interface RetryReadyOptions {
	/** Total probe attempts before giving up. Defaults to 60. */
	attempts?: number;
	/** Delay before the second attempt, doubled each retry. Defaults to 1000ms. */
	baseDelayMs?: number;
	/** Upper bound for a single delay. Defaults to 5000ms. */
	maxDelayMs?: number;
	/** Injectable sleep for tests. Defaults to `setTimeout`. */
	sleep?: (ms: number) => Promise<void>;
	/** Receives each failed attempt and the delay before the next one. */
	onRetry?: (error: unknown, attempt: number, delayMs: number) => void;
}

const defaultSleep = (ms: number) =>
	new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Run `probe` until it resolves. Retries a failed probe with bounded backoff and
 * rejects with the last error once the attempt budget is exhausted. A
 * non-positive attempt count is treated as a single attempt.
 */
export async function retryReady(
	probe: () => Promise<void>,
	options: RetryReadyOptions = {},
): Promise<void> {
	const attempts = Math.max(1, options.attempts ?? 60);
	const baseDelayMs = Math.max(0, options.baseDelayMs ?? 1000);
	const maxDelayMs = Math.max(0, options.maxDelayMs ?? 5000);
	const sleep = options.sleep ?? defaultSleep;
	let lastError: unknown;
	for (let attempt = 1; attempt <= attempts; attempt += 1) {
		try {
			await probe();
			return;
		} catch (error) {
			lastError = error;
			if (attempt === attempts) break;
			const delayMs = Math.min(baseDelayMs * 2 ** (attempt - 1), maxDelayMs);
			options.onRetry?.(error, attempt, delayMs);
			await sleep(delayMs);
		}
	}
	throw lastError;
}
