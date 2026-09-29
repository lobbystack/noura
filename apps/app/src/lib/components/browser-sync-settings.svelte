<script lang="ts">
	import { onMount } from 'svelte';
	import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
	import { Badge } from '$lib/components/ui/badge';
	import { getBrowserSyncController } from '$lib/browser-sync';
	import { statusLabels } from './browser-sync/copy';
	import DeviceAccess from './browser-sync/device-access.svelte';
	import { BrowserSyncPanel } from './browser-sync/panel.svelte';
	import RecoveryKit from './browser-sync/recovery-kit.svelte';
	import SyncConflicts from './browser-sync/sync-conflicts.svelte';
	import WorkspaceDevices from './browser-sync/workspace-devices.svelte';
	import WorkspaceSync from './browser-sync/workspace-sync.svelte';

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

	const panel = new BrowserSyncPanel(() => ({ workspaceId, workspaceFiles }));

	// Device review needs an unlocked browser and sync turned on.
	const canReviewDevices = $derived(
		panel.unlocked && panel.summary?.configured === true,
	);

	onMount(() => {
		if (!canUseBrowserSync) return;
		let disposed = false;
		void (async () => {
			const controller = await getBrowserSyncController();
			if (!disposed) await panel.connect(controller);
		})();
		return () => {
			disposed = true;
		};
	});
</script>

{#if canUseBrowserSync}
	<section class="flex flex-col gap-4" aria-label="Browser sync">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<h3 class="text-sm font-medium">This browser</h3>
			<Badge variant={panel.status === 'error' ? 'destructive' : 'secondary'}
				>{statusLabels[panel.status]}</Badge
			>
		</div>
		<p class="text-xs text-muted-foreground">
			Your passphrase protects the sync key stored in this browser.
		</p>

		{#if !panel.controller}
			<p role="status" class="text-xs text-muted-foreground">
				Checking this browser…
			</p>
		{:else if panel.status === 'unavailable'}
			<p class="text-sm text-muted-foreground">
				This browser can't store sync keys safely, so sync is off here.
			</p>
		{:else}
			<DeviceAccess {panel} />
			{#key panel.session}
				<WorkspaceSync {panel} />
				<RecoveryKit {panel} />
				{#if canReviewDevices}
					<WorkspaceDevices {panel} />
				{/if}
				<SyncConflicts {panel} />
			{/key}
		{/if}

		{#if panel.notice}<p role="status" class="text-sm">{panel.notice}</p>{/if}
		{#if panel.error}
			<p role="alert" class="text-sm text-destructive">{panel.error}</p>
		{/if}
	</section>
{/if}
