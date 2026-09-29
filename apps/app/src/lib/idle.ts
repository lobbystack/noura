/**
 * Run work when the main thread is idle, or after `timeoutMs` at the latest.
 * Returns a function that cancels the work if it has not started.
 */
export function whenIdle(callback: () => void, timeoutMs = 2_000): () => void {
	if (typeof requestIdleCallback === 'function') {
		const handle = requestIdleCallback(() => callback(), {
			timeout: timeoutMs,
		});
		return () => cancelIdleCallback(handle);
	}
	const timer = setTimeout(callback, Math.min(timeoutMs, 200));
	return () => clearTimeout(timer);
}
