/**
 * App wiring for browser sync. The controller and its stores live in
 * `@noura/browser-sync-client`; this module keeps one controller per tab.
 */

import { createSameOriginFetch } from '@noura/browser-sync';
import {
	createBrowserSyncController,
	type BrowserSyncController,
} from '@noura/browser-sync-client';

export {
	extractEmbeddedRecoveryIdentity,
	type RecoveryKitFile,
} from '@noura/browser-sync';
export {
	hasBrowserSyncBinding,
	MAX_BROWSER_ATTACHMENT_BYTES,
	openBrowserSyncPluginMarker,
	type BrowserSyncConflictDetail,
	type BrowserSyncController,
	type BrowserSyncDeviceCard,
	type BrowserSyncStatus,
	type BrowserSyncWorkspaceSummary,
	type SyncNowOutcome,
} from '@noura/browser-sync-client';

let sharedController: Promise<BrowserSyncController> | undefined;

/**
 * The app-wide controller. The account page and the workspace shell share one
 * in-memory unlock state for this tab. It only sends same-origin requests.
 */
export function getBrowserSyncController(): Promise<BrowserSyncController> {
	const origin = globalThis.location?.origin ?? '';
	sharedController ??= createBrowserSyncController({
		origin,
		fetch: createSameOriginFetch(origin),
	});
	return sharedController;
}

/** Drop the shared controller so the next caller rebuilds it (sign-out). */
export function resetBrowserSyncController(): void {
	sharedController = undefined;
}
