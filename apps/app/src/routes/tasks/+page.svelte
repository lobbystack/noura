<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import KanbanBoard from '$lib/components/kanban-board.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as ToggleGroup from '$lib/components/ui/toggle-group/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import ListBullets from 'phosphor-svelte/lib/ListBullets';
	import Kanban from 'phosphor-svelte/lib/Kanban';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';

	type Task = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['tasks']['list']>
	>[number];

	let tasks = $state<Task[]>([]);
	let loading = $state(true);
	let selectedTask = $state<Task | null>(null);
	let inspectorOpen = $state(false);
	let view = $state<'list' | 'board'>('list');

	async function load() {
		try {
			loading = true;
			tasks = await getNouraClient().tasks.list();
		} finally {
			loading = false;
		}
	}

	async function select(task: Task) {
		selectedTask = task;
		inspectorOpen = true;
		tabsStore.open(task.id, 'task', task.title);
	}

	async function create() {
		const res = await getNouraClient().tasks.create({ title: 'New task' });
		await load();
		select(res.value as Task);
	}

	async function toggleDone(task: Task) {
		const done = task.properties?.status === 'done';
		await getNouraClient().tasks[done ? 'reopen' : 'complete']({
			id: task.id,
			expectedRevision: task.revision ?? '',
		});
		await load();
	}

	onMount(() => {
		if (browser) load();
	});
</script>

<PageHeader title="Tasks" description="All work items">
	{#snippet actions()}
		<ToggleGroup.Root
			bind:value={
				() => view,
				(value) => {
					if (value === 'list' || value === 'board') view = value;
				}
			}
			type="single"
			variant="outline"
			size="sm"
			class="mr-1"
		>
			<ToggleGroup.Item value="list" aria-label="List view">
				<ListBullets />
			</ToggleGroup.Item>
			<ToggleGroup.Item value="board" aria-label="Board view">
				<Kanban />
			</ToggleGroup.Item>
		</ToggleGroup.Root>
		<Button size="sm" onclick={create}>
			<Plus data-icon="inline-start" />
			Add task
		</Button>
	{/snippet}
</PageHeader>

{#if view === 'board'}
	<KanbanBoard />
{:else if loading}
	<div class="flex flex-col gap-1 p-2">
		{#each [0, 1, 2, 3] as i (i)}
			<Skeleton class="h-10 w-full" />
		{/each}
	</div>
{:else if tasks.length === 0}
	<Empty.Root class="flex-1">
		<Empty.Media variant="icon">
			<FolderOpen />
		</Empty.Media>
		<Empty.Header>
			<Empty.Title>No tasks yet</Empty.Title>
			<Empty.Description
				>Create your first task to get started.</Empty.Description
			>
		</Empty.Header>
		<Empty.Content>
			<Button onclick={create}>
				<Plus data-icon="inline-start" />
				Create task
			</Button>
		</Empty.Content>
	</Empty.Root>
{:else}
	<div class="flex-1 overflow-y-auto">
		<div class="divide-y divide-border/60">
			{#each tasks as task (task.id)}
				{@const done = task.properties?.status === 'done'}
				<button
					class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-muted/50"
					onclick={() => select(task)}
				>
					<input
						type="checkbox"
						checked={done}
						class="size-4 rounded border-input text-primary focus:ring-primary"
						onclick={(event) => {
							event.stopPropagation();
							toggleDone(task);
						}}
					/>
					<span
						class={done
							? 'flex-1 text-sm font-medium line-through text-muted-foreground'
							: 'flex-1 text-sm font-medium'}
					>
						{task.title}
					</span>
					{#if task.properties?.project}
						<Badge variant="secondary" class="text-xs">
							{task.properties.project}
						</Badge>
					{/if}
					{#if task.properties?.due}
						<span class="text-xs text-muted-foreground whitespace-nowrap">
							{task.properties.due}
						</span>
					{/if}
				</button>
			{/each}
		</div>
	</div>
{/if}

{#if view === 'list'}
	<ObjectInspector
		bind:open={inspectorOpen}
		object={selectedTask}
		onclose={() => {
			selectedTask = null;
		}}
	/>
{/if}
