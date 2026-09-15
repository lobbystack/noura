<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import * as Field from '$lib/components/ui/field';
	import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
	import {
		getBrowserSyncController,
		type BrowserSyncController,
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
			return `Sync complete. Pushed ${outcome.pushed}, applied ${outcome.applied}, ${outcome.conflicts} conflict(s). Cursor ${outcome.cursor}.`;
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
		void refresh();
	}

	function syncNow() {
		const value = controller;
		if (!value || busy) return;
		void run(async () => {
			const outcome = await value.syncNow();
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
				<p id="browser-sync-durability" class="text-xs text-muted-foreground">
					Browser storage is a disposable replica, not a backup. Clearing site
					data or browser eviction can erase this workspace and its sync state.
					Keep a canonical copy on a native device. JSON backups are not
					encrypted; store them securely.
				</p>
			</div>
		{/if}

		{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
		{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
	</section>
{/if}
