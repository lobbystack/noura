<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { toast } from 'svelte-sonner';
	import type { Snippet } from 'svelte';

	let {
		title = $bindable(''),
		description,
		actions,
		searchValue = $bindable(),
		searchPlaceholder = 'Search…',
	}: {
		title?: string;
		description?: string;
		actions?: Snippet;
		searchValue?: string;
		searchPlaceholder?: string;
	} = $props();
</script>

<div class="flex h-14 shrink-0 items-center gap-4 border-b border-border px-6">
	<div class="min-w-0 flex-1">
		<h1 class="text-sm font-semibold">{title}</h1>
		{#if description}
			<p class="truncate text-xs text-muted-foreground">{description}</p>
		{/if}
	</div>
	{#if searchValue !== undefined}
		<div class="w-56">
			<Input
				placeholder={searchPlaceholder}
				bind:value={searchValue}
				type="search"
				class="h-8"
			/>
		</div>
	{/if}
	{@render actions?.()}
</div>
