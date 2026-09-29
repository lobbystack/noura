/**
 * @noura/browser-sync-client — the browser sync controller and its stores.
 *
 * This package joins `@noura/browser-sync` (device custody, enrollment, key
 * delivery, operation encryption, recovery kits) with
 * `@noura/browser-sync-engine` (the local replica engine) for the browser
 * build:
 *
 * - {@link BrowserSyncController} and {@link createBrowserSyncController} hold
 *   the unlocked device identity and object keys in tab memory and drive
 *   enrollment, workspace binding, reconcile, device review, conflicts,
 *   attachments, and recovery kits.
 * - Origin-private (OPFS) stores keep the wrapped device bundle, the binding
 *   record, the engine state, and the sync plugin marker outside canonical
 *   workspace files.
 * - {@link runBrowserSyncReconcile}, {@link runBrowserSyncResolveConflict}, and
 *   {@link runBrowserSyncSendAttachment} run one engine pass over injected
 *   boundaries.
 *
 * It imports no SvelteKit or app modules. The app creates the controller and
 * keeps one per tab.
 */

export {
	BrowserSyncController,
	createBrowserSyncController,
	DEFAULT_BROWSER_SYNC_BUNDLE_ID,
} from './controller';
export type {
	BrowserSyncControllerOptions,
	ImportNativeRecoveryKitOptions,
} from './controller';

export type {
	BrowserSyncConflictDetail,
	BrowserSyncDeviceCard,
	BrowserSyncDeviceInfo,
	BrowserSyncFailureCode,
	BrowserSyncObjectBinding,
	BrowserSyncReconcileInput,
	BrowserSyncResult,
	BrowserSyncStatus,
	BrowserSyncWorkspaceBinding,
	BrowserSyncWorkspaceSummary,
	SyncNowOutcome,
} from './types';

export { createBrowserSyncRemote } from './remote';
export {
	createBrowserAttachmentFetcher,
	MAX_BROWSER_ATTACHMENT_BYTES,
} from './attachment-fetcher';
export {
	runBrowserSyncReconcile,
	runBrowserSyncResolveConflict,
} from './reconcile';
export type { BrowserSyncReconcileOutcome } from './reconcile';
export { runBrowserSyncSendAttachment } from './attachments';
export type {
	BrowserSyncSendAttachmentInput,
	BrowserSyncSendAttachmentOutcome,
} from './attachments';
export { createBrowserSyncWorkspaceBinding } from './workspace-binding';
export type { BrowserSyncWorkspaceSource } from './workspace-binding';

export {
	hasBrowserSyncBinding,
	openBrowserSyncBindingStore,
	openBrowserSyncKeyStore,
	openBrowserSyncPluginMarker,
	openBrowserSyncStateStore,
} from './opfs';

export { migrateBrowserSyncPlugin, SYNC_PLUGIN_ID } from './plugin-migration';
export type {
	BrowserSyncPluginMarker,
	BrowserSyncPluginMigration,
} from './plugin-migration';
