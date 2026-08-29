<script lang="ts">
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { FolderOpen, Plus } from 'phosphor-svelte';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';

	type Task = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['tasks']['list']>
	>[number];

	let tasks = $state<Task[]>([]);
	let loading = $state(true);
	let selectedTask = $state<Task | null>(null);
	let inspectorOpen = $state(false);

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
		<button
			class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium hover:bg-muted"
			onclick={create}
		>
			<Plus data-icon="inline-start" />
			Add task
		</button>
	{/snippet}
</PageHeader>

{#if loading}
	<div class="space-y-1 p-2">
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
			<button
				class="rounded-md px-3 py-1.5 text-sm font-medium bg-primary text-primary-foreground hover:bg-primary/90"
				onclick={create}>Create task</button
			>
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
						onclick={(e) => {
							e.stopPropagation();
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

<ObjectInspector
	bind:open={inspectorOpen}
	object={selectedTask}
	onclose={() => {
		selectedTask = null;
		tabsStore.setActive('null');
	}}
/>
