<script lang="ts">
	import { Badge } from '$lib/components/ui/badge/index.js';

	let {
		entries,
	}: {
		entries: readonly {
			title: string;
			content: string;
			sourceId: string;
		}[];
	} = $props();

	const groups = $derived.by(() => {
		const result: Array<{
			date: string;
			entries: Array<{ title: string; content: string; sourceId: string }>;
		}> = [];
		for (const entry of entries) {
			const date = entry.content.slice(0, 10);
			const group = result.find((candidate) => candidate.date === date);
			if (group) group.entries.push(entry);
			else result.push({ date, entries: [entry] });
		}
		return result;
	});

	function entryType(content: string): string {
		return /\(([^)]+)\)\s*$/.exec(content)?.[1] ?? 'object';
	}
</script>

<section aria-label="Upcoming calendar" class="flex flex-col gap-4 p-2">
	<p class="text-sm text-muted-foreground">
		Read-only view of outstanding dated tasks and projects. This browser
		workspace derives it from objects and does not use AI.
	</p>
	{#if entries.length === 0}
		<p class="text-sm text-muted-foreground">
			No outstanding dated tasks or projects in the next 30 days.
		</p>
	{:else}
		{#each groups as group (group.date)}
			<section class="flex flex-col gap-1">
				<h2 class="text-sm font-medium">{group.date}</h2>
				<ul class="flex flex-col gap-1">
					{#each group.entries as entry (entry.sourceId)}
						<li
							class="flex items-center gap-2 rounded border border-border/60 px-2 py-1 text-sm"
						>
							<Badge variant="outline">{entryType(entry.content)}</Badge>
							<span class="truncate">{entry.title}</span>
						</li>
					{/each}
				</ul>
			</section>
		{/each}
	{/if}
</section>
