<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import { count, syncOutcomeText } from './copy';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let action = $state<'enable' | 'sync' | null>(null);
	let skipped = $state(0);

	const summary = $derived(panel.summary);
	const canEnable = $derived(
		panel.status === 'enrolled' &&
			!!panel.workspaceId &&
			!!panel.workspaceFiles,
	);

	const summaryText = $derived.by(() => {
		if (!summary) return '';
		if (summary.pending === 0 && summary.conflicts === 0)
			return 'No changes waiting.';
		const parts = [count(summary.pending, 'change waiting', 'changes waiting')];
		if (summary.conflicts > 0)
			parts.push(count(summary.conflicts, 'conflict', 'conflicts'));
		return `${parts.join(' · ')}.`;
	});

	function enable() {
		const workspaceId = panel.workspaceId;
		const workspaceFiles = panel.workspaceFiles;
		if (!workspaceId || !workspaceFiles) return;
		action = 'enable';
		void panel
			.run(async (controller) => {
				const result = await controller.enableSync({
					workspaceId,
					workspaceFiles,
				});
				if (!result.ok) {
					panel.error = result.message;
					return;
				}
				return 'Sync is on. Choose Sync now to send your changes.';
			})
			.finally(() => {
				action = null;
			});
	}

	function syncNow() {
		action = 'sync';
		void panel
			.run(async (controller) => {
				const outcome = await controller.syncNow();
				skipped = outcome.status === 'synced' ? outcome.skippedUnmanaged : 0;
				if (outcome.status !== 'synced') {
					panel.error = syncOutcomeText(outcome);
					return;
				}
				return syncOutcomeText(outcome);
			})
			.finally(() => {
				action = null;
			});
	}
</script>

<div class="flex flex-col gap-2" aria-labelledby="browser-sync-heading">
	<h4 id="browser-sync-heading" class="text-sm font-medium">Workspace sync</h4>
	{#if summary?.configured}
		<p role="status" class="text-xs text-muted-foreground">{summaryText}</p>
	{:else if panel.workspaceId && panel.workspaceFiles}
		<p role="status" class="text-xs text-muted-foreground">
			Turn on sync to share this workspace, encrypted, with your other devices.
		</p>
	{:else}
		<p role="status" class="text-xs text-muted-foreground">
			Open a workspace to sync it.
		</p>
	{/if}
	<div class="flex flex-wrap gap-2">
		{#if !summary?.configured}
			<Button
				variant="outline"
				disabled={panel.busy || !canEnable}
				onclick={enable}
			>
				{#if action === 'enable'}
					<Spinner data-icon="inline-start" />Turning on…
				{:else}
					Turn on sync
				{/if}
			</Button>
		{/if}
		<Button
			variant="outline"
			disabled={panel.busy ||
				panel.status !== 'enrolled' ||
				!summary?.configured}
			onclick={syncNow}
		>
			{#if action === 'sync'}
				<Spinner data-icon="inline-start" />Syncing…
			{:else}
				Sync now
			{/if}
		</Button>
	</div>
	{#if skipped > 0}
		<p role="status" class="text-xs text-muted-foreground">
			Skipped {count(skipped, 'file', 'files')} that aren't notes, tasks, or projects.
		</p>
	{/if}
	<p class="text-xs text-muted-foreground">
		Clearing site data erases this workspace from the browser, so keep a copy on
		your computer.
	</p>
</div>
