<script lang="ts">
	import { diagnostics } from '$lib/state.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Tray from 'phosphor-svelte/lib/Tray';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	onMount(() => {
		if (browser) diagnostics.refresh();
	});
</script>

<div class="flex h-14 shrink-0 items-center gap-2 border-b border-border px-6">
	<h1 class="text-sm font-semibold">Inbox</h1>
	<Badge variant="secondary" class="h-5 gap-0.5 px-1.5 text-[0.6rem]"
		>{diagnostics.issues.length}</Badge
	>
</div>

{#if diagnostics.issues.length === 0}
	<EmptyState
		icon={Tray}
		title="No issues"
		description="All good. Nothing requires your attention."
	/>
{:else}
	<div class="flex-1 overflow-y-auto">
		<div class="divide-y divide-border/60">
			{#each diagnostics.issues as issue (issue.code)}
				<div class="flex items-start gap-4 px-4 py-3">
					<Badge variant="destructive" class="mt-0.5 shrink-0 text-xs"
						>{issue.code}</Badge
					>
					<div class="flex-1">
						<p class="text-sm">{issue.message}</p>
						{#if issue.relativePath}<p
								class="mt-0.5 text-xs font-mono text-muted-foreground"
							>
								{issue.relativePath}
							</p>{/if}
					</div>
				</div>
			{/each}
		</div>
	</div>
{/if}
