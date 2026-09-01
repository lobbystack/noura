<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import ProjectBoard from '$lib/components/project-board.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import ArrowLeft from 'phosphor-svelte/lib/ArrowLeft';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	type Project = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['projects']['list']>
	>[number];

	let projects = $state<Project[]>([]);
	let tasks = $state<Record<string, number>>({});
	let loading = $state(true);
	let selected = $state<Project | null>(null);

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

	function select(p: Project) {
		selected = p;
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

	onMount(() => {
		if (browser) load();
	});
</script>

{#if !selected}
	<PageHeader title="Projects" description="All projects">
		{#snippet actions()}
			<Button size="sm" onclick={create}>
				<Plus data-icon="inline-start" /> New project
			</Button>
		{/snippet}
	</PageHeader>

	{#if loading}
		<div class="flex flex-col gap-1 p-2">
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
									variant={statusColor(
										String(p.properties?.status ?? 'planned'),
									)}
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
{:else}
	<div
		class="flex h-14 shrink-0 items-center gap-3 border-b border-border px-4"
	>
		<Button variant="ghost" size="sm" onclick={() => (selected = null)}>
			<ArrowLeft data-icon="inline-start" />
			Projects
		</Button>
		<div class="min-w-0 flex-1">
			<div class="flex items-center gap-2">
				<h1 class="truncate text-sm font-semibold">{selected.title}</h1>
				<Badge
					variant={statusColor(
						String(selected.properties?.status ?? 'planned'),
					)}
					class="text-xs capitalize"
				>
					{String(selected.properties?.status ?? 'planned')}
				</Badge>
			</div>
		</div>
	</div>
	<ProjectBoard projectId={selected.id} projectTitle={selected.title} />
{/if}
