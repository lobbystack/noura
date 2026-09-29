<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		description,
		actions,
	}: {
		title: string;
		/** A short line under the title, such as the current view or month. */
		description?: string;
		actions?: Snippet;
	} = $props();
</script>

<div class="flex h-14 shrink-0 items-center gap-4 border-b border-border px-6">
	<div class="min-w-0 flex-1">
		<h1 class="text-sm font-semibold">{title}</h1>
		{#if description}
			<p class="truncate text-xs text-muted-foreground">{description}</p>
		{/if}
	</div>
	{@render actions?.()}
</div>
