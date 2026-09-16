<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as Field from '$lib/components/ui/field';
	import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
	import type { RecoveryKitFile } from '@noura/browser-sync';
	import {
		extractEmbeddedRecoveryIdentity,
		getBrowserSyncController,
		type BrowserSyncConflictDetail,
		type BrowserSyncController,
		type BrowserSyncDeviceCard,
		type BrowserSyncStatus,
		type BrowserSyncWorkspaceSummary,
		type SyncNowOutcome,
	} from '$lib/browser-sync';

	let {
		workspaceId = null,
		workspaceFiles = null,
	}: {
		workspaceId?: string | null;
		workspaceFiles?: BrowserWorkspaceFiles | null;
	} = $props();

	// Tauri-free web detection: the account route must not pull in native code.
	const canUseBrowserSync =
		typeof window !== 'undefined' &&
		typeof Worker !== 'undefined' &&
		!('__TAURI_INTERNALS__' in window);

	let controller = $state<BrowserSyncController | null>(null);
	let status = $state<BrowserSyncStatus>('unavailable');
	let deviceId = $state<string | null>(null);
	let enrolled = $state(false);
	let summary = $state<BrowserSyncWorkspaceSummary | null>(null);
	let passphrase = $state('');
	let busy = $state(false);
	let error = $state('');
	let notice = $state('');
	let reconcile = $state('');
	let skippedUnmanaged = $state(0);
	let devices = $state<BrowserSyncDeviceCard[]>([]);
	let devicesLoaded = $state(false);
	let devicesBusy = $state(false);
	let devicesError = $state('');
	let verified = $state<Record<string, string>>({});
	let conflictBusy = $state<string | null>(null);
	let conflictError = $state('');
	let kitPassphrase = $state('');
	let importPassphrase = $state('');
	let kitFile = $state<File | null>(null);
	let kitAction = $state<'export' | 'import' | null>(null);
	let kitError = $state('');
	let kitNotice = $state('');
	let nativeKitFile = $state<File | null>(null);
	let nativeRecoveryIdentity = $state('');
	let nativeIdentityEmbedded = $state(false);
	let nativeKitAction = $state<'import' | null>(null);
	let nativeKitError = $state('');
	let nativeKitNotice = $state('');

	const statusLabels: Record<BrowserSyncStatus, string> = {
		unavailable: 'Unavailable in this browser',
		locked: 'Locked',
		unlocked: 'Unlocked, not enrolled',
		enrolled: 'Enrolled',
		error: 'Needs attention',
	};

	function friendly(code: string, message: string): string {
		if (code === 'passphrase_rejected')
			return 'That passphrase did not unlock this browser device.';
		return message || 'The browser sync operation failed.';
	}

	function syncMessage(outcome: SyncNowOutcome): string {
		if (outcome.status === 'synced') {
			const skipped =
				outcome.skippedUnmanaged > 0
					? ` Skipped ${outcome.skippedUnmanaged} file(s) with no owning object.`
					: '';
			return `Sync complete. Pushed ${outcome.pushed}, applied ${outcome.applied}, ${outcome.conflicts} conflict(s). Cursor ${outcome.cursor}.${skipped}`;
		}
		return outcome.message;
	}

	async function refresh() {
		const value = controller;
		if (!value) return;
		status = value.status();
		const device = value.device();
		deviceId = device?.deviceId ?? null;
		enrolled = device?.enrolled ?? false;
		summary = await value.workspaceSummary();
	}

	const canEnable = $derived(
		status === 'enrolled' && !!workspaceId && !!workspaceFiles,
	);

	// Device review needs an unlocked device and an existing durable binding.
	const canReviewDevices = $derived(
		(status === 'unlocked' || status === 'enrolled') &&
			summary?.configured === true,
	);

	const conflicts = $derived(summary?.conflictDetails ?? []);

	// Recovery kits need unlocked custody; the key bundle must be readable.
	const custodyActive = $derived(
		status === 'unlocked' || status === 'enrolled',
	);
	const kitBusy = $derived(kitAction !== null);
	const canExportKit = $derived(custodyActive && !!kitPassphrase && !kitBusy);
	// A workspace id is required to re-key the recovered binding on import.
	const canImportKit = $derived(
		!!workspaceId && !!kitFile && !!importPassphrase && !kitBusy,
	);
	const nativeKitBusy = $derived(nativeKitAction !== null);
	// Native import needs unlocked custody, an open workspace, and an identity.
	const canImportNativeKit = $derived(
		custodyActive &&
			!!workspaceId &&
			!!nativeKitFile &&
			nativeRecoveryIdentity.trim().length > 0 &&
			!nativeKitBusy,
	);

	async function restoreBinding() {
		const value = controller;
		if (!value || !workspaceId || !workspaceFiles) return;
		if (!value.device()?.enrolled) return;
		await value.resumeSync({ workspaceId, workspaceFiles });
	}

	async function run(action: () => Promise<void>) {
		if (busy) return;
		busy = true;
		error = '';
		notice = '';
		try {
			await action();
		} catch (cause) {
			error =
				cause instanceof Error
					? cause.message
					: 'The browser sync operation failed.';
		} finally {
			busy = false;
			await refresh();
		}
	}

	function enroll() {
		const value = controller;
		if (!value || busy) return;
		void run(async () => {
			const result = await value.enroll({ passphrase });
			if (!result.ok) {
				error = friendly(result.code, result.message);
				return;
			}
			passphrase = '';
			notice =
				'This browser is enrolled. Ask a trusted device to approve its fingerprint and deliver workspace keys.';
		});
	}

	async function unlock() {
		const value = controller;
		if (!value || busy) return;
		void run(async () => {
			const result = await value.unlock({ passphrase });
			if (!result.ok) {
				error = friendly(result.code, result.message);
				return;
			}
			passphrase = '';
			notice =
				result.value === 'enrolled'
					? 'Browser device unlocked for this tab.'
					: 'Browser device unlocked. It is not enrolled with the sync server yet.';
			await restoreBinding();
		});
	}

	function lock() {
		const value = controller;
		if (!value || busy) return;
		value.lock();
		passphrase = '';
		error = '';
		notice = 'Browser device locked. Key material was dropped from this tab.';
		reconcile = '';
		skippedUnmanaged = 0;
		devices = [];
		devicesLoaded = false;
		devicesError = '';
		verified = {};
		conflictError = '';
		void refresh();
	}

	function syncNow() {
		const value = controller;
		if (!value || busy) return;
		void run(async () => {
			const outcome = await value.syncNow();
			skippedUnmanaged =
				outcome.status === 'synced' ? outcome.skippedUnmanaged : 0;
			reconcile = syncMessage(outcome);
			if (outcome.status === 'synced') {
				notice = reconcile;
				return;
			}
			error = reconcile;
		});
	}

	function enable() {
		const value = controller;
		if (!value || busy || !workspaceId || !workspaceFiles) return;
		void run(async () => {
			const result = await value.enableSync({ workspaceId, workspaceFiles });
			if (!result.ok) {
				error = friendly(result.code, result.message);
				return;
			}
			notice =
				'Encrypted sync is enabled for this browser workspace. Run “Sync now” to reconcile.';
		});
	}

	function workspaceScope(): { workspaceId?: string } {
		return workspaceId ? { workspaceId } : {};
	}

	async function loadDevices(): Promise<void> {
		const value = controller;
		if (!value) return;
		const result = await value.listWorkspaceDevices(workspaceScope());
		if (!result.ok) {
			devices = [];
			devicesError = friendly(result.code, result.message);
			return;
		}
		devices = result.value;
		// Drop verifications for cards that are gone or whose fingerprint changed.
		const stillVerified: Record<string, string> = {};
		for (const device of devices) {
			if (verified[device.deviceId] === device.fingerprint) {
				stillVerified[device.deviceId] = device.fingerprint;
			}
		}
		verified = stillVerified;
	}

	function checkDevices() {
		const value = controller;
		if (!value || devicesBusy) return;
		void (async () => {
			devicesBusy = true;
			devicesError = '';
			try {
				await loadDevices();
			} catch (cause) {
				devices = [];
				devicesError =
					cause instanceof Error
						? cause.message
						: 'The workspace device list could not be loaded.';
			} finally {
				devicesLoaded = true;
				devicesBusy = false;
			}
		})();
	}

	function approve(device: BrowserSyncDeviceCard) {
		const value = controller;
		if (!value || devicesBusy) return;
		void (async () => {
			devicesBusy = true;
			devicesError = '';
			notice = '';
			try {
				// Pass the fingerprint exactly as it was displayed and verified.
				const result = await value.approveDevice(
					device.deviceId,
					device.fingerprint,
				);
				if (!result.ok) {
					devicesError = friendly(result.code, result.message);
					return;
				}
				await loadDevices();
				notice = `Approved ${device.deviceId}. Its signing key is now pinned on this browser.`;
			} catch (cause) {
				devicesError =
					cause instanceof Error
						? cause.message
						: 'Approving the device failed.';
			} finally {
				devicesBusy = false;
			}
		})();
	}

	function revoke(device: BrowserSyncDeviceCard) {
		const value = controller;
		if (!value || devicesBusy) return;
		void (async () => {
			devicesBusy = true;
			devicesError = '';
			notice = '';
			try {
				const result = await value.revokeDeviceApproval(device.deviceId);
				if (!result.ok) {
					devicesError = friendly(result.code, result.message);
					return;
				}
				await loadDevices();
				notice = result.value
					? `Removed the local approval for ${device.deviceId}.`
					: `${device.deviceId} was not approved on this browser.`;
			} catch (cause) {
				devicesError =
					cause instanceof Error
						? cause.message
						: 'Revoking the device approval failed.';
			} finally {
				devicesBusy = false;
			}
		})();
	}

	function resolveConflictChoice(
		conflict: BrowserSyncConflictDetail,
		choice: 'local' | 'remote',
	) {
		const value = controller;
		if (!value || conflictBusy) return;
		void (async () => {
			conflictBusy = conflict.operationId;
			conflictError = '';
			notice = '';
			try {
				const result = await value.resolveConflict(
					conflict.operationId,
					choice,
				);
				if (!result.ok) {
					conflictError = friendly(result.code, result.message);
					return;
				}
				await refresh();
				const kept = choice === 'local' ? 'local' : 'remote';
				notice = `Kept the ${kept} version of ${conflict.path}. ${result.value.remaining} conflict(s) remain.`;
			} catch (cause) {
				conflictError =
					cause instanceof Error
						? cause.message
						: 'Resolving the conflict failed.';
			} finally {
				conflictBusy = null;
			}
		})();
	}

	function kitMessage(code: string, message: string): string {
		if (code === 'passphrase_rejected')
			return 'That passphrase did not decrypt this recovery kit.';
		return message || 'The recovery kit operation failed.';
	}

	/** Build a safe download name for an exported kit. */
	function recoveryKitFilename(): string {
		const label = (deviceId ?? workspaceId ?? 'browser')
			.replace(/[^a-zA-Z0-9_-]+/g, '-')
			.replace(/^-+|-+$/g, '')
			.slice(0, 64);
		return `noura-recovery-kit-${label || 'browser'}.noura-recovery-kit.json`;
	}

	/** Download an exported kit and release the object URL once consumed. */
	function downloadRecoveryKit(kit: RecoveryKitFile): void {
		const blob = new Blob([JSON.stringify(kit)], {
			type: 'application/json',
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = recoveryKitFilename();
		anchor.hidden = true;
		document.body.append(anchor);
		try {
			anchor.click();
		} finally {
			anchor.remove();
			// Give the browser time to start the download before revoking.
			setTimeout(() => URL.revokeObjectURL(url), 60_000);
		}
	}

	function exportKit() {
		const value = controller;
		if (!value || kitBusy || !kitPassphrase) return;
		void (async () => {
			kitAction = 'export';
			kitError = '';
			kitNotice = '';
			try {
				const result = await value.exportRecoveryKit(kitPassphrase);
				if (!result.ok) {
					kitError = kitMessage(result.code, result.message);
					return;
				}
				downloadRecoveryKit(result.value);
				kitPassphrase = '';
				kitNotice =
					'Recovery kit downloaded. It contains this browser’s device credentials; store it in an encrypted, private location. This browser kit is not the native Noura recovery format.';
			} catch (cause) {
				kitError =
					cause instanceof Error
						? cause.message
						: 'The recovery kit could not be exported.';
			} finally {
				kitAction = null;
			}
		})();
	}

	function chooseRecoveryKit(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		kitFile = input.files?.[0] ?? null;
		// Clear the native value so the same file can be re-selected later.
		input.value = '';
		kitError = '';
		kitNotice = '';
	}

	function importKit() {
		const value = controller;
		if (!value || kitBusy) return;
		if (!workspaceId) {
			kitError = 'Open a browser workspace before importing a recovery kit.';
			return;
		}
		const file = kitFile;
		if (!file) {
			kitError = 'Choose a recovery kit file to import.';
			return;
		}
		if (!importPassphrase) {
			kitError = 'Enter the recovery kit passphrase.';
			return;
		}
		void (async () => {
			kitAction = 'import';
			kitError = '';
			kitNotice = '';
			try {
				let parsed: unknown;
				try {
					parsed = JSON.parse(await file.text());
				} catch {
					kitError =
						'That file is not valid JSON. Choose the .noura-recovery-kit.json file you exported.';
					return;
				}
				const result = await value.importRecoveryKit(
					parsed,
					importPassphrase,
					workspaceId,
				);
				if (!result.ok) {
					kitError = kitMessage(result.code, result.message);
					return;
				}
				importPassphrase = '';
				kitFile = null;
				kitNotice =
					'Recovery kit imported. This browser now holds the kit’s device identity and stays locked until you unlock it with that device’s passphrase.';
				await refresh();
			} catch (cause) {
				kitError =
					cause instanceof Error
						? cause.message
						: 'The recovery kit could not be imported.';
			} finally {
				kitAction = null;
			}
		})();
	}

	/**
	 * Read a native kit and prefill the recovery identity when the kit embeds
	 * one, locking the field so a different value cannot be silently substituted.
	 */
	async function chooseNativeRecoveryKit(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0] ?? null;
		// Clear the native value so the same file can be re-selected later.
		input.value = '';
		nativeKitFile = file;
		nativeKitError = '';
		nativeKitNotice = '';
		nativeIdentityEmbedded = false;
		nativeRecoveryIdentity = '';
		if (!file) return;
		try {
			const parsed: unknown = JSON.parse(await file.text());
			const embedded = extractEmbeddedRecoveryIdentity(parsed);
			nativeIdentityEmbedded = embedded !== null;
			nativeRecoveryIdentity = embedded ?? '';
		} catch (cause) {
			nativeKitFile = null;
			nativeKitError =
				cause instanceof Error
					? cause.message
					: 'That file is not a native Noura recovery kit.';
		}
	}

	function importNativeKit() {
		const value = controller;
		if (!value || nativeKitBusy) return;
		if (!workspaceId) {
			nativeKitError =
				'Open a browser workspace before importing a native recovery kit.';
			return;
		}
		const file = nativeKitFile;
		if (!file) {
			nativeKitError = 'Choose a native recovery kit file to import.';
			return;
		}
		const recoveryIdentity = nativeRecoveryIdentity.trim();
		if (!recoveryIdentity) {
			nativeKitError =
				'Paste the recovery identity from the kit; it is used only for this import.';
			return;
		}
		void (async () => {
			nativeKitAction = 'import';
			nativeKitError = '';
			nativeKitNotice = '';
			try {
				let parsed: unknown;
				try {
					parsed = JSON.parse(await file.text());
				} catch {
					nativeKitError =
						'That file is not valid JSON. Choose the native kit exported by a Noura device.';
					return;
				}
				const result = await value.importNativeRecoveryKit(parsed, {
					recoveryIdentity,
					localWorkspaceId: workspaceId,
				});
				if (!result.ok) {
					nativeKitError = kitMessage(result.code, result.message);
					return;
				}
				// Drop the secret from the form as soon as it is used.
				nativeKitFile = null;
				nativeIdentityEmbedded = false;
				nativeRecoveryIdentity = '';
				nativeKitNotice =
					'Recovered the object keys from the native kit and re-wrapped them to this browser device. Plaintext keys and the recovery identity were not stored.';
				await refresh();
			} catch (cause) {
				nativeKitError =
					cause instanceof Error
						? cause.message
						: 'The native recovery kit could not be imported.';
			} finally {
				nativeKitAction = null;
			}
		})();
	}

	onMount(() => {
		if (!canUseBrowserSync) return;
		let disposed = false;
		void (async () => {
			const value = await getBrowserSyncController();
			if (disposed) return;
			controller = value;
			value.setBinding(null);
			if (workspaceId && workspaceFiles && value.device()?.enrolled)
				await value.resumeSync({ workspaceId, workspaceFiles });
			if (!disposed) await refresh();
		})();
		return () => {
			disposed = true;
		};
	});
</script>

{#if canUseBrowserSync}
	<section
		class="flex flex-col gap-4"
		aria-label="Browser device and encrypted sync"
	>
		<div class="flex flex-wrap items-center justify-between gap-2">
			<h3 class="text-sm font-medium">This browser</h3>
			<Badge variant={status === 'error' ? 'destructive' : 'secondary'}
				>{statusLabels[status]}</Badge
			>
		</div>
		<p class="text-xs text-muted-foreground">
			Browser sync keeps only a passphrase-wrapped device key on this device.
			Unwrapped keys live in this tab’s memory until you lock or close it. The
			hosted browser client is a convenience client, never the high-assurance
			one.
		</p>

		{#if !controller}
			<p role="status" class="text-xs text-muted-foreground">
				Checking this browser’s sync support…
			</p>
		{:else if status === 'unavailable'}
			<p class="text-sm text-muted-foreground">
				This browser does not provide the origin-private storage Noura needs to
				protect a device key. Browser sync is unavailable here.
			</p>
		{:else}
			{#if deviceId}
				<div class="flex flex-col gap-1">
					<p class="text-xs font-medium">Browser device ID</p>
					<p class="break-all font-mono text-xs select-text">{deviceId}</p>
				</div>
			{/if}

			{#if !enrolled}
				<form
					onsubmit={(event) => {
						event.preventDefault();
						if (deviceId) unlock();
						else enroll();
					}}
				>
					<Field.Group>
						<Field.Field>
							<Field.Label for="browser-sync-passphrase"
								>Device passphrase</Field.Label
							>
							<Input
								id="browser-sync-passphrase"
								type="password"
								autocomplete="current-password"
								bind:value={passphrase}
								disabled={busy}
								aria-describedby="browser-sync-passphrase-help"
							/>
							<p
								id="browser-sync-passphrase-help"
								class="text-xs text-muted-foreground"
							>
								The passphrase is never stored. It derives the key that unwraps
								this browser’s device bundle.
							</p>
						</Field.Field>
						<div class="flex flex-wrap gap-2">
							{#if deviceId}
								<Button type="submit" disabled={busy || !passphrase}
									>{busy ? 'Unlocking…' : 'Unlock this browser'}</Button
								>
								<Button
									type="button"
									variant="outline"
									disabled={busy || !passphrase}
									onclick={enroll}>Enroll as a new browser device</Button
								>
							{:else}
								<Button type="submit" disabled={busy || !passphrase}
									>{busy ? 'Enrolling…' : 'Enroll this browser'}</Button
								>
							{/if}
						</div>
					</Field.Group>
				</form>
			{:else}
				<div class="flex flex-wrap gap-2">
					<Button variant="outline" disabled={busy} onclick={lock}
						>Lock this browser</Button
					>
				</div>
			{/if}

			<div class="flex flex-col gap-2" aria-labelledby="browser-sync-heading">
				<h4 id="browser-sync-heading" class="text-sm font-medium">
					Workspace sync
				</h4>
				{#if summary?.configured}
					<p role="status" class="text-xs text-muted-foreground">
						Workspace {summary.workspaceId} · cursor {summary.cursor} ·
						{summary.pending} pending · {summary.conflicts} conflicts
					</p>
				{:else if workspaceId && workspaceFiles}
					<p role="status" class="text-xs text-muted-foreground">
						Encrypted sync is not enabled for this browser workspace yet.
						Enabling it creates a remote workspace and a self-wrapped object
						key; canonical workspace files stay on this device.
					</p>
				{:else}
					<p role="status" class="text-xs text-muted-foreground">
						Open a browser workspace to enable sync. Nothing was uploaded or
						changed.
					</p>
				{/if}
				<div class="flex flex-wrap gap-2">
					{#if !summary?.configured}
						<Button
							variant="outline"
							disabled={busy || !canEnable}
							onclick={enable}
							>{busy ? 'Enabling…' : 'Enable sync for this workspace'}</Button
						>
					{/if}
					<Button
						variant="outline"
						disabled={busy || status !== 'enrolled' || !summary?.configured}
						onclick={syncNow}>{busy ? 'Syncing…' : 'Sync now'}</Button
					>
				</div>
				{#if reconcile}
					<p role="status" class="text-xs text-muted-foreground">{reconcile}</p>
				{/if}
				{#if skippedUnmanaged > 0}
					<p role="status" class="text-xs text-muted-foreground">
						{skippedUnmanaged} file(s) have no workspace object id, so they are not
						synchronized yet. Give each file a managed note, task, or project object
						before it can sync.
					</p>
				{/if}
				<p id="browser-sync-durability" class="text-xs text-muted-foreground">
					Browser storage is a disposable replica, not a backup. Clearing site
					data or browser eviction can erase this workspace and its sync state.
					Keep a canonical copy on a native device. JSON backups are not
					encrypted; store them securely.
				</p>
			</div>

			{#if status === 'locked' || custodyActive || kitNotice || kitError || nativeKitNotice || nativeKitError}
				<div
					class="flex flex-col gap-3"
					aria-labelledby="browser-sync-recovery-heading"
				>
					<h4 id="browser-sync-recovery-heading" class="text-sm font-medium">
						Recovery kit
					</h4>
					{#if custodyActive}
						<p class="text-xs text-muted-foreground">
							A recovery kit is an encrypted copy of this browser’s device
							credentials. It restores the same device identity, so it can
							restore sync access on another browser. Treat it as a secret:
							store it only in an encrypted, private location. This browser kit
							format is not interchangeable with the native Noura recovery
							format.
						</p>

						<div class="flex flex-col gap-3 rounded-xl border p-4">
							<h5 class="text-xs font-medium">Export a recovery kit</h5>
							<Field.Field>
								<Field.Label for="browser-sync-recovery-kit-passphrase"
									>Recovery kit passphrase</Field.Label
								>
								<Input
									id="browser-sync-recovery-kit-passphrase"
									type="password"
									autocomplete="new-password"
									bind:value={kitPassphrase}
									disabled={kitBusy}
									aria-describedby="browser-sync-recovery-kit-passphrase-help"
								/>
								<p
									id="browser-sync-recovery-kit-passphrase-help"
									class="text-xs text-muted-foreground"
								>
									This passphrase encrypts the kit. It is not stored, and you
									need it to restore the kit later.
								</p>
							</Field.Field>
							<div>
								<Button
									type="button"
									variant="outline"
									disabled={!canExportKit}
									onclick={exportKit}
									>{kitAction === 'export'
										? 'Exporting…'
										: 'Export recovery kit'}</Button
								>
							</div>
						</div>
					{/if}

					<div class="flex flex-col gap-3 rounded-xl border p-4">
						<h5 class="text-xs font-medium">Import a recovery kit</h5>
						<p class="text-xs text-muted-foreground">
							Importing restores the kit’s device credentials on this browser,
							including on a fresh browser where this device was never enrolled.
							Open a browser workspace first so the recovered binding has a
							local workspace to attach to.
						</p>
						<Field.Field>
							<Field.Label for="browser-sync-recovery-kit-file"
								>Recovery kit file</Field.Label
							>
							<Input
								id="browser-sync-recovery-kit-file"
								type="file"
								accept=".noura-recovery-kit.json,application/json"
								disabled={kitBusy}
								onchange={chooseRecoveryKit}
								aria-describedby="browser-sync-recovery-kit-file-help"
							/>
							<p
								id="browser-sync-recovery-kit-file-help"
								class="text-xs text-muted-foreground"
							>
								{#if kitFile}
									Selected {kitFile.name}.
								{:else}
									Choose the JSON recovery kit file exported from another
									browser.
								{/if}
							</p>
						</Field.Field>
						<Field.Field>
							<Field.Label for="browser-sync-recovery-kit-import-passphrase"
								>Recovery kit passphrase</Field.Label
							>
							<Input
								id="browser-sync-recovery-kit-import-passphrase"
								type="password"
								autocomplete="current-password"
								bind:value={importPassphrase}
								disabled={kitBusy || !workspaceId}
								aria-describedby="browser-sync-recovery-kit-import-help"
							/>
							<p
								id="browser-sync-recovery-kit-import-help"
								class="text-xs text-muted-foreground"
							>
								{#if workspaceId}
									The passphrase that was set when the kit was exported.
								{:else}
									Import is disabled because no browser workspace is open; a
									workspace id is required to re-key the binding.
								{/if}
							</p>
						</Field.Field>
						<div>
							<Button
								type="button"
								variant="outline"
								disabled={!canImportKit}
								onclick={importKit}
								>{kitAction === 'import'
									? 'Importing…'
									: 'Import recovery kit'}</Button
							>
						</div>
					</div>

					<div class="flex flex-col gap-3 rounded-xl border p-4">
						<h5 class="text-xs font-medium">Import a native recovery kit</h5>
						<p class="text-xs text-muted-foreground">
							This recovers object keys from a native Noura
							<code>noura.sync.recovery</code> kit and re-wraps them to this browser
							device. It is not the browser kit above. The recovery identity is sensitive:
							it is used only in this tab and is never stored or uploaded.
						</p>
						<Field.Field>
							<Field.Label for="browser-sync-native-kit-file"
								>Native recovery kit file</Field.Label
							>
							<Input
								id="browser-sync-native-kit-file"
								type="file"
								accept=".json,application/json"
								disabled={nativeKitBusy}
								onchange={chooseNativeRecoveryKit}
								aria-describedby="browser-sync-native-kit-file-help"
							/>
							<p
								id="browser-sync-native-kit-file-help"
								class="text-xs text-muted-foreground"
							>
								{#if nativeKitFile}
									Selected {nativeKitFile.name}.
								{:else}
									Choose the JSON native recovery kit exported by a native Noura
									device.
								{/if}
							</p>
						</Field.Field>
						<Field.Field>
							<Field.Label for="browser-sync-native-recovery-identity"
								>Recovery identity</Field.Label
							>
							<Input
								id="browser-sync-native-recovery-identity"
								type="password"
								autocomplete="off"
								bind:value={nativeRecoveryIdentity}
								disabled={nativeKitBusy ||
									nativeIdentityEmbedded ||
									!workspaceId}
								aria-describedby="browser-sync-native-recovery-identity-help"
							/>
							<p
								id="browser-sync-native-recovery-identity-help"
								class="text-xs text-muted-foreground"
							>
								{#if nativeIdentityEmbedded}
									This kit embeds its recovery identity, so the field is filled
									in and locked. It is used only to unwrap the kit’s object
									keys.
								{:else if !workspaceId}
									Open a browser workspace before importing; a workspace id is
									required to attach the recovered binding.
								{:else}
									Paste the <code>AGE-SECRET-KEY-…</code> recovery identity that this
									kit does not embed. Treat it as a secret; it is never stored.
								{/if}
							</p>
						</Field.Field>
						<div>
							<Button
								type="button"
								variant="outline"
								disabled={!canImportNativeKit}
								onclick={importNativeKit}
								>{nativeKitAction === 'import'
									? 'Importing…'
									: 'Import native recovery kit'}</Button
							>
						</div>
					</div>
					{#if nativeKitNotice}
						<p role="status" class="text-sm">{nativeKitNotice}</p>
					{/if}
					{#if nativeKitError}
						<p role="alert" class="text-sm text-destructive">
							{nativeKitError}
						</p>
					{/if}

					{#if kitNotice}
						<p role="status" class="text-sm">{kitNotice}</p>
					{/if}
					{#if kitError}
						<p role="alert" class="text-sm text-destructive">{kitError}</p>
					{/if}
				</div>
			{/if}

			{#if canReviewDevices}
				<div
					class="flex flex-col gap-3"
					aria-labelledby="browser-sync-devices-heading"
				>
					<h4 id="browser-sync-devices-heading" class="text-sm font-medium">
						Workspace devices
					</h4>
					<p class="text-xs text-muted-foreground">
						Approving a device pins its signing key on this browser. After that,
						only this browser plus the devices you approved here are trusted to
						sign workspace changes. Compare the full fingerprint with the other
						device before approving. Revoking removes only the local approval.
					</p>
					<div>
						<Button
							variant="outline"
							disabled={busy || devicesBusy}
							onclick={checkDevices}
							>{devicesBusy && !devicesLoaded
								? 'Checking devices…'
								: 'Check workspace devices'}</Button
						>
					</div>
					{#if devicesError}
						<p role="alert" class="text-sm text-destructive">{devicesError}</p>
					{/if}
					{#if devicesLoaded}
						{#if devices.length > 0}
							<ul class="flex flex-col gap-3">
								{#each devices as device (device.deviceId)}
									<li class="flex flex-col gap-3 rounded-xl border p-4">
										<div
											class="flex flex-wrap items-center justify-between gap-2"
										>
											<div class="flex flex-col gap-1">
												<p class="text-xs font-medium">Device id</p>
												<p class="break-all font-mono text-xs select-text">
													{device.deviceId}
												</p>
											</div>
											{#if device.approved}
												<Badge variant="secondary">Approved</Badge>
											{:else}
												<Badge variant="outline">Not approved</Badge>
											{/if}
										</div>
										<div class="flex flex-col gap-1">
											<p class="text-xs font-medium">Fingerprint</p>
											<p class="break-all font-mono text-xs select-text">
												{device.fingerprint}
											</p>
										</div>
										{#if device.approved}
											<div>
												<Button
													variant="outline"
													disabled={busy || devicesBusy}
													onclick={() => revoke(device)}>Revoke approval</Button
												>
											</div>
										{:else}
											<Field.FieldGroup>
												<Field.Field orientation="horizontal">
													<Checkbox
														id={`browser-sync-verify-${device.deviceId}`}
														checked={verified[device.deviceId] ===
															device.fingerprint}
														onCheckedChange={(checked) => {
															verified[device.deviceId] = checked
																? device.fingerprint
																: '';
														}}
														disabled={busy || devicesBusy}
													/>
													<Field.FieldLabel
														for={`browser-sync-verify-${device.deviceId}`}
														>I compared the full fingerprint on the other device
														and it matches.</Field.FieldLabel
													>
												</Field.Field>
											</Field.FieldGroup>
											<div>
												<Button
													disabled={busy ||
														devicesBusy ||
														verified[device.deviceId] !== device.fingerprint}
													onclick={() => approve(device)}>Approve device</Button
												>
											</div>
										{/if}
									</li>
								{/each}
							</ul>
						{:else}
							<p role="status" class="text-xs text-muted-foreground">
								No workspace devices were returned.
							</p>
						{/if}
					{/if}
				</div>
			{/if}

			{#if conflicts.length > 0}
				<div
					class="flex flex-col gap-3"
					aria-labelledby="browser-sync-conflicts-heading"
				>
					<h4 id="browser-sync-conflicts-heading" class="text-sm font-medium">
						Conflicts
					</h4>
					<p class="text-xs text-muted-foreground">
						Sync recorded {conflicts.length} change(s) that could not apply automatically.
						Nothing is overwritten until you choose. Keep local keeps the bytes on
						this browser; Take remote applies the remote version. Each choice is explicit
						and applies to one conflict.
					</p>
					{#if conflictError}
						<p role="alert" class="text-sm text-destructive">{conflictError}</p>
					{/if}
					<ul class="flex flex-col gap-3">
						{#each conflicts as conflict (conflict.operationId)}
							<li class="flex flex-col gap-3 rounded-xl border p-4">
								<div class="flex flex-col gap-1">
									<p class="break-all font-mono text-xs select-text">
										{conflict.path}
									</p>
									<p class="text-xs text-muted-foreground">
										{conflict.reason}
									</p>
								</div>
								<div class="flex flex-wrap gap-2">
									<Button
										variant="outline"
										disabled={busy || conflictBusy !== null}
										onclick={() => resolveConflictChoice(conflict, 'local')}
										>{conflictBusy === conflict.operationId
											? 'Resolving…'
											: 'Keep local'}</Button
									>
									<Button
										variant="outline"
										disabled={busy || conflictBusy !== null}
										onclick={() => resolveConflictChoice(conflict, 'remote')}
										>{conflictBusy === conflict.operationId
											? 'Resolving…'
											: 'Take remote'}</Button
									>
								</div>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
		{/if}

		{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
		{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
	</section>
{/if}
