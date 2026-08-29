<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import MagnifyingGlass from 'phosphor-svelte/lib/MagnifyingGlass';
	import EmptyState from '$lib/components/empty-state.svelte';

	let query = $state('');
	let results = $state<any[]>([]);
	let loading = $state(false);

	async function search() {
		if (!query.trim()) {
			results = [];
			return;
		}
		loading = true;
		results = await getNouraClient().search.query({ query, limit: 20 });
		loading = false;
	}
</script>

<div class="flex h-14 shrink-0 items-center gap-4 border-b border-border px-6">
	<h1 class="text-sm font-semibold">Search</h1>
	<div class="flex-1 max-w-md">
		<Input
			placeholder="Search across notes, tasks, projects…"
			bind:value={query}
			onkeydown={(e) => {
				if (e.key === 'Enter') search();
			}}
			class="h-8"
		/>
	</div>
</div>

{#if !query.trim()}
	<EmptyState
		icon={MagnifyingGlass}
		title="Search"
		description="Enter a query to search the workspace."
	/>
{:else if loading}
	<div class="p-4 text-sm text-muted-foreground">Searching…</div>
{:else if results.length === 0}
	<div class="p-4 text-sm text-muted-foreground">No results for “{query}”.</div>
{:else}
	<div class="flex-1 overflow-y-auto">
		<div class="divide-y divide-border/60">
			{#each results as r (r.objectId ?? r.relativePath)}
				<button class="w-full px-4 py-3 text-left hover:bg-muted/50">
					<div class="flex items-center gap-2">
						{#if r.objectType}<Badge variant="outline" class="text-xs"
								>{r.objectType}</Badge
							>{/if}
						<span class="font-medium truncate">{r.title}</span>
					</div>
					{#if r.snippet}
						<p class="mt-1 line-clamp-2 pl-0 text-xs text-muted-foreground">
							{r.snippet}
						</p>
					{/if}
					<p class="mt-0.5 text-xs text-muted-foreground/70 truncate">
						{r.relativePath}
					</p>
				</button>
			{/each}
		</div>
	</div>
{/if}
