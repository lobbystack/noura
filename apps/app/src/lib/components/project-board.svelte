<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';

	let { projectId, projectTitle } = $props<{
		projectId: string;
		projectTitle: string;
	}>();

	type Task = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['tasks']['list']>
	>[number];

	type Group = { id: string; title: string; items: Task[] };

	let groups = $state<Group[]>([]);
	let loading = $state(true);
	let selectedTask = $state<Task | null>(null);
	let inspectorOpen = $state(false);
	let dragTask = $state<Task | null>(null);
	let dragOverColumn = $state<string | null>(null);
	let dragOverTaskId = $state<string | null>(null);

	const columnLabel = (id: string) =>
		id
			.split('-')
			.map((part) => part.charAt(0).toUpperCase() + part.slice(1))
			.join(' ');

	const isEmpty = $derived(groups.every((g) => g.items.length === 0));

	async function load() {
		try {
			loading = true;
			const board = await getNouraClient().kanban.getBoard({ projectId });
			groups = board.groups;
		} finally {
			loading = false;
		}
	}

	function select(task: Task) {
		selectedTask = task;
		inspectorOpen = true;
		tabsStore.open(task.id, 'task', task.title);
	}

	async function create() {
		await getNouraClient().tasks.create({ title: 'New task' });
		await load();
	}

	function handleDragStart(event: DragEvent, task: Task) {
		if (!event.dataTransfer) return;
		dragTask = task;
		event.dataTransfer.effectAllowed = 'move';
		event.dataTransfer.setData('text/plain', task.id);
	}

	function handleDragOver(event: DragEvent, columnId: string, overId?: string) {
		if (!dragTask || !event.dataTransfer) return;
		event.preventDefault();
		event.dataTransfer.dropEffect = 'move';
		dragOverColumn = columnId;
		dragOverTaskId = overId ?? null;
	}

	function handleDragLeave(columnId: string, overId?: string) {
		if (dragOverColumn === columnId) {
			if (overId === undefined || dragOverTaskId === overId) {
				dragOverColumn = null;
				dragOverTaskId = null;
			}
		}
	}

	function clearDrag() {
		dragTask = null;
		dragOverColumn = null;
		dragOverTaskId = null;
	}

	async function handleDrop(
		event: DragEvent,
		columnId: string,
		beforeId?: string,
	) {
		if (!dragTask) return;
		event.preventDefault();
		const task = dragTask;
		clearDrag();
		const sourceStatus =
			(typeof task.properties?.status === 'string' && task.properties.status) ||
			'todo';
		if (columnId === sourceStatus && beforeId === task.id) return;
		if (columnId === sourceStatus && !beforeId) return;
		const group = groups.find((entry) => entry.id === columnId);
		const beforeIndex = beforeId
			? (group?.items.findIndex((item) => item.id === beforeId) ?? -1)
			: -1;
		const afterId =
			beforeIndex > 0
				? group?.items[beforeIndex - 1]?.id
				: !beforeId && group && group.items.length > 0
					? group.items[group.items.length - 1]?.id
					: undefined;
		await getNouraClient().kanban.moveTask({
			taskId: task.id,
			status: columnId as never,
			beforeId,
			afterId,
			expectedRevision: task.revision ?? '',
		});
		await load();
	}

	onMount(() => {
		if (browser) load();
	});
</script>

{#if loading}
	<div class="flex flex-1 gap-px overflow-x-auto bg-border/40 p-px">
		{#each [0, 1, 2, 3] as i (i)}
			<div class="flex-1 bg-background p-2">
				<Skeleton class="h-5 w-24" />
				<div class="mt-3 flex flex-col gap-2">
					<Skeleton class="h-10 w-full" />
					<Skeleton class="h-10 w-full" />
				</div>
			</div>
		{/each}
	</div>
{:else if isEmpty}
	<Empty.Root class="flex-1">
		<Empty.Media variant="icon">
			<FolderOpen />
		</Empty.Media>
		<Empty.Header>
			<Empty.Title>No tasks in this project</Empty.Title>
			<Empty.Description>
				Tasks assigned to {projectTitle} will appear on this board.
			</Empty.Description>
		</Empty.Header>
		<Empty.Content>
			<Button onclick={create}>
				<Plus data-icon="inline-start" />
				Add task
			</Button>
		</Empty.Content>
	</Empty.Root>
{:else}
	<div
		class="flex flex-1 items-stretch gap-px overflow-x-auto border-y border-border/60 bg-border/60"
	>
		{#each groups as group (group.id)}
			<section
				class={`flex w-64 shrink-0 flex-col bg-background transition-colors ${dragOverColumn === group.id ? 'bg-muted/50' : ''}`}
				ondragover={(event) => handleDragOver(event, group.id)}
				ondragleave={() => handleDragLeave(group.id)}
				ondrop={(event) => handleDrop(event, group.id)}
				aria-label={`${columnLabel(group.id)} column`}
			>
				<header
					class="flex items-center justify-between border-b border-border/60 px-3 py-2.5"
				>
					<h3
						class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground"
					>
						{columnLabel(group.id)}
					</h3>
					<span class="text-[11px] text-muted-foreground"
						>{group.items.length}</span
					>
				</header>
				<div
					class="flex flex-1 flex-col gap-px overflow-y-auto bg-border/40 p-px"
				>
					{#each group.items as task (task.id)}
						{@const done = task.properties?.status === 'done'}
						<button
							class={`block w-full bg-background px-3 py-2.5 text-left transition-colors hover:bg-muted/70 ${dragTask?.id === task.id ? 'opacity-40' : ''} ${dragOverTaskId === task.id ? 'ring-1 ring-inset ring-primary/40' : ''}`}
							draggable="true"
							ondragstart={(event) => handleDragStart(event, task)}
							ondragend={clearDrag}
							ondragover={(event) => handleDragOver(event, group.id, task.id)}
							ondragleave={() => handleDragLeave(group.id, task.id)}
							ondrop={(event) => handleDrop(event, group.id, task.id)}
							onclick={() => select(task)}
						>
							<span
								class={`block text-sm font-medium ${done ? 'line-through text-muted-foreground' : ''}`}
							>
								{task.title}
							</span>
							{#if task.properties?.due}
								<span class="mt-1 text-[10px] text-muted-foreground"
									>{task.properties.due}</span
								>
							{/if}
						</button>
					{/each}
				</div>
			</section>
		{/each}
	</div>
{/if}

<ObjectInspector
	bind:open={inspectorOpen}
	object={selectedTask}
	onclose={() => {
		selectedTask = null;
	}}
/>
