<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { NotePencil, Plus } from 'phosphor-svelte';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['list']>
	>[number];

	let notes = $state<Note[]>([]);
	let loading = $state(true);
	let selected = $state<Note | null>(null);
	let inspectorOpen = $state(false);

	async function load() {
		try {
			loading = true;
			notes = await getNouraClient().notes.list();
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		if (browser) load();
	});

	async function select(note: Note) {
		selected = note;
		inspectorOpen = true;
		tabsStore.open(note.id, 'note', note.title);
	}

	async function create() {
		const res = await getNouraClient().notes.create({ title: 'Untitled' });
		await load();
		select(res.value as Note);
	}
</script>

<PageHeader title="Notes" description="All notes">
	{#snippet actions()}
		<button
			class="flex items-center gap-1.5 px-3 py-1.5 text-sm hover:bg-muted rounded-md"
			onclick={create}
		>
			<Plus data-icon="inline-start" /> New note
		</button>
	{/snippet}
</PageHeader>

{#if loading}
	<div class="space-y-1 p-2">
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
			{#each notes as note (note.id)}
				<button
					class="w-full px-4 py-3 text-left hover:bg-muted/50"
					onclick={() => select(note)}
				>
					<div class="flex items-center gap-2">
						<NotePencil class="size-4 shrink-0 text-muted-foreground" />
						<span class="truncate text-sm font-medium">{note.title}</span>
					</div>
					{#if note.body}
						<p class="mt-0.5 line-clamp-1 pl-6 text-xs text-muted-foreground">
							{note.body}
						</p>
					{/if}
				</button>
			{/each}
		</div>
	</div>
{/if}

<ObjectInspector
	bind:open={inspectorOpen}
	object={selected}
	onclose={() => {
		selected = null;
	}}
/>
