<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { FolderOpen, Plus } from 'phosphor-svelte';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	type Project = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['projects']['list']>
	>[number];

	let projects = $state<Project[]>([]);
	let tasks = $state<Record<string, number>>({});
	let loading = $state(true);
	let selected = $state<Project | null>(null);
	let inspectorOpen = $state(false);

	async function load() {
		try {
			loading = true;
			projects = await getNouraClient().projects.list();
			const all = await getNouraClient().tasks.list();
			const map: Record<string, number> = {};
			for (const t of all) {
				if (typeof t.properties?.project === 'string')
					map[t.properties.project] = (map[t.properties.project] ?? 0) + 1;
			}
			tasks = map;
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		if (browser) load();
	});

	async function select(p: Project) {
		selected = p;
		inspectorOpen = true;
		tabsStore.open(p.id, 'project', p.title);
	}

	async function create() {
		const res = await getNouraClient().projects.create({
			title: 'New project',
		});
		await load();
		select(res.value as Project);
	}

	const statusColor = (s: string) =>
		s === 'active'
			? 'default'
			: s === 'completed'
				? 'secondary'
				: s === 'on-hold'
					? 'outline'
					: 'secondary';
</script>

<PageHeader title="Projects" description="All projects">
	{#snippet actions()}
		<button
			class="flex items-center gap-1.5 px-3 py-1.5 text-sm hover:bg-muted rounded-md"
			onclick={create}
		>
			<Plus data-icon="inline-start" /> New project
		</button>
	{/snippet}
</PageHeader>

{#if loading}
	<div class="space-y-1 p-2">
		{#each [0, 1, 2, 3] as i (i)}
			<Skeleton class="h-16 w-full" />
		{/each}
	</div>
{:else if projects.length === 0}
	<EmptyState
		icon={FolderOpen}
		title="No projects"
		description="Organize work with projects."
		actionLabel="New project"
		onAction={create}
	/>
{:else}
	<div class="flex-1 overflow-y-auto">
		<div class="divide-y divide-border/60">
			{#each projects as p (p.id)}
				{@const count = tasks[p.id] ?? 0}
				<button
					class="flex w-full items-center gap-4 px-4 py-3 text-left hover:bg-muted/50"
					onclick={() => select(p)}
				>
					<div class="min-w-0 flex-1">
						<div class="flex items-center gap-2">
							<span class="truncate text-sm font-medium">{p.title}</span>
							<Badge
								variant={statusColor(String(p.properties?.status ?? 'planned'))}
								class="text-xs capitalize"
							>
								{String(p.properties?.status ?? 'planned')}
							</Badge>
						</div>
						{#if p.body}
							<p class="mt-0.5 line-clamp-1 text-xs text-muted-foreground">
								{p.body}
							</p>
						{/if}
					</div>
					<span class="shrink-0 text-xs text-muted-foreground"
						>{count} task{count === 1 ? '' : 's'}</span
					>
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
