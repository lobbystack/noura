<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import { cn } from '$lib/utils';
	import PageHeader from '$lib/components/page-header.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['list']>
	>[number];

	let notes = $state<Note[]>([]);
	let loading = $state(true);
	let selected = $state<Note | null>(null);
	async function select(n: Note) {
		if (selected?.id !== n.id && !(await flushPendingDrafts())) return;
		selected = n;
		tabsStore.open(n.id, 'note', n.title);
	}

	async function load() {
		try {
			loading = true;
			const list = await getNouraClient().notes.list();
			notes = list;
			// Pre-select first note if one exists and nothing is selected yet
			if (!selected && list.length > 0) await select(list[0]);
		} finally {
			loading = false;
		}
	}

	async function create() {
		if (!(await flushPendingDrafts())) return;
		const res = await getNouraClient().notes.create({ title: 'Untitled' });
		await load();
		if (res.value) await select(res.value as Note);
	}

	function handleSaved(updated: Note) {
		// Refresh the list so the sidebar reflects renames
		notes = notes.map((n) => (n.id === updated.id ? updated : n));
		selected = updated;
	}

	onMount(() => {
		if (browser) load();
	});
</script>

<div class="flex h-screen">
	<aside
		class="flex w-72 shrink-0 flex-col border-r border-border bg-background"
	>
		<PageHeader title="Notes" description={`${notes.length} notes`}>
			{#snippet actions()}
				<Button size="sm" onclick={create}>
					<Plus data-icon="inline-start" />
					New note
				</Button>
			{/snippet}
		</PageHeader>

		{#if loading}
			<div class="flex flex-col gap-1 p-2">
				{#each [0, 1, 2, 3] as i (i)}
					<Skeleton class="h-10 w-full" />
				{/each}
			</div>
		{:else if notes.length === 0}
			<EmptyState
				icon={NotePencil}
				title="No notes"
				description="Capture your first thought."
				actionLabel="Create note"
				onAction={create}
			/>
		{:else}
			<div class="flex-1 overflow-y-auto">
				<div class="divide-y divide-border/60">
					{#each notes as n (n.id)}
						<button
							class={cn(
								'w-full px-4 py-3 text-left hover:bg-muted/50',
								selected?.id === n.id && 'bg-muted/60',
							)}
							onclick={() => void select(n)}
						>
							<div class="flex items-center gap-2">
								<NotePencil
									class={cn(
										'size-4 shrink-0',
										selected?.id === n.id
											? 'text-foreground'
											: 'text-muted-foreground',
									)}
								/>
								<span
									class={cn(
										'truncate text-sm',
										selected?.id === n.id
											? 'font-semibold text-foreground'
											: 'font-medium',
									)}
								>
									{n.title}
								</span>
							</div>
							{#if n.body}
								<p
									class="mt-0.5 line-clamp-1 pl-6 text-xs text-muted-foreground"
								>
									{n.body}
								</p>
							{/if}
						</button>
					{/each}
				</div>
			</div>
		{/if}
	</aside>

	<main class="flex min-w-0 flex-1 flex-col">
		{#key selected?.id}
			<NoteEditor note={selected} onsaved={handleSaved} />
		{/key}
	</main>
</div>
done
