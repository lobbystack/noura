export interface HostCloseRequest {
	preventDefault(): void;
}

export interface HostLifecycleAdapter {
	onCloseRequested(
		handler: (event: HostCloseRequest) => void | Promise<void>,
	): Promise<() => void>;
	forceClose(): Promise<void>;
}

/**
 * Keep the host window alive until all registered canonical-file writes have
 * completed. Failed flushes deliberately leave the window open.
 */
export function installPendingDraftCloseGuard(
	host: HostLifecycleAdapter,
	flush: () => Promise<boolean>,
): Promise<() => void> {
	let forcingClose = false;
	return host.onCloseRequested(async (event) => {
		if (forcingClose) return;
		event.preventDefault();
		if (!(await flush())) return;
		forcingClose = true;
		await host.forceClose();
	});
}
