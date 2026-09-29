/** The engine's remote boundary over the bearer-token sync transport. */

import {
	BrowserSyncTransport,
	createSameOriginFetch,
	type FetchLike,
} from '@noura/browser-sync';
import type { BrowserSyncRemote } from '@noura/browser-sync-engine';

/**
 * Build the engine's remote boundary from the bearer-token transport. Kept here
 * so the same wiring is reused when a host can bind a workspace replica.
 */
export function createBrowserSyncRemote(options: {
	origin: string;
	token: string;
	workspaceId: string;
	fetch?: FetchLike;
}): BrowserSyncRemote {
	const transport = new BrowserSyncTransport({
		origin: options.origin,
		token: options.token,
		fetch: options.fetch ?? createSameOriginFetch(options.origin),
	});
	return {
		async push(operations) {
			const sequences = await transport.push(options.workspaceId, operations);
			return { sequences };
		},
		pull(cursor) {
			return transport.pull(options.workspaceId, cursor);
		},
	};
}
