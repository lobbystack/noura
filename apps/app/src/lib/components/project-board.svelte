<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { toast } from 'svelte-sonner';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import X from 'phosphor-svelte/lib/X';
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
	let detailOpen = $state(false);
	let closingDetail = $state(false);
	let projects = $state<
		Awaited<ReturnType<ReturnType<typeof getNouraClient>['projects']['list']>>
	>([]);
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
			const [board, projectList] = await Promise.all([
				getNouraClient().kanban.getBoard({ projectId }),
				getNouraClient().projects.list(),
			]);
			groups = board.groups;
			projects = projectList;
		} finally {
			loading = false;
		}
	}

	function select(task: Task) {
		selectedTask = task;
		detailOpen = true;
		tabsStore.open(task.id, 'task', task.title);
	}

	async function create() {
		const result = await getNouraClient().tasks.create({
			title: 'New task',
			properties: { project: projectId, status: 'todo', priority: 'medium' },
		});
		await load();
		select(result.value as Task);
	}

	async function moveTaskFromMenu(task: Task, status: string) {
		try {
			await getNouraClient().kanban.moveTask({
				taskId: task.id,
				status: status as never,
				expectedRevision: task.revision,
			});
		} catch {
			toast.error('Could not move the task', {
				description:
					'The task changed while you were viewing it. Refreshed to the latest state.',
			});
		}
		await load();
	}

	async function closeDetail() {
		if (closingDetail) return;
		closingDetail = true;
		try {
			if (await flushPendingDrafts()) detailOpen = false;
		} finally {
			closingDetail = false;
		}
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
		try {
			await getNouraClient().kanban.moveTask({
				taskId: task.id,
				status: columnId as never,
				beforeId,
				afterId,
				expectedRevision: task.revision ?? '',
			});
		} catch {
			// A stale revision is expected after edits land through another
			// surface; reloading shows the authoritative column state.
			toast.error('Could not move the task', {
				description:
					'The task changed while you were viewing it. Refreshed to the latest state.',
			});
		}
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
						<div
							role="listitem"
							class={`flex items-start bg-background transition-colors hover:bg-muted/70 ${dragTask?.id === task.id ? 'opacity-40' : ''} ${dragOverTaskId === task.id ? 'ring-1 ring-inset ring-primary/40' : ''}`}
							draggable="true"
							ondragstart={(event) => handleDragStart(event, task)}
							ondragend={clearDrag}
							ondragover={(event) => handleDragOver(event, group.id, task.id)}
							ondragleave={() => handleDragLeave(group.id, task.id)}
							ondrop={(event) => handleDrop(event, group.id, task.id)}
						>
							<button
								class="min-w-0 flex-1 px-3 py-2.5 text-left"
								onclick={() => select(task)}
								><span
									class={`block text-sm font-medium ${done ? 'line-through text-muted-foreground' : ''}`}
									>{task.title}</span
								>{#if task.properties?.due}<span
										class="mt-1 text-[10px] text-muted-foreground"
										>{task.properties.due}</span
									>{/if}</button
							>
							<DropdownMenu.Root
								><DropdownMenu.Trigger
									><Button
										variant="ghost"
										size="icon-sm"
										aria-label="Move {task.title}">•••</Button
									></DropdownMenu.Trigger
								><DropdownMenu.Content
									><DropdownMenu.Group
										><DropdownMenu.Label>Move to</DropdownMenu.Label
										>{#each groups as destination (destination.id)}<DropdownMenu.Item
												onclick={() =>
													void moveTaskFromMenu(task, destination.id)}
												>{columnLabel(destination.id)}</DropdownMenu.Item
											>{/each}</DropdownMenu.Group
									></DropdownMenu.Content
								></DropdownMenu.Root
							>
						</div>
					{/each}
				</div>
			</section>
		{/each}
	</div>
{/if}

<Sheet.Root bind:open={detailOpen}>
	<Sheet.Content
		class="flex w-full flex-col p-0 sm:max-w-4xl"
		showCloseButton={false}
		onEscapeKeydown={(event) => {
			event.preventDefault();
			void closeDetail();
		}}
		onInteractOutside={(event) => {
			event.preventDefault();
			void closeDetail();
		}}
	>
		<Sheet.Header class="sr-only"
			><Sheet.Title>Task detail</Sheet.Title><Sheet.Description
				>Edit the selected project task.</Sheet.Description
			></Sheet.Header
		>
		<Button
			variant="ghost"
			class="absolute top-4 right-4 bg-secondary"
			size="icon-sm"
			disabled={closingDetail}
			onclick={() => void closeDetail()}
			aria-label="Close task detail"><X /></Button
		>
		{#if selectedTask}{#key selectedTask.id}<TaskDetail
					task={selectedTask}
					{projects}
					onupdated={() => void load()}
				/>{/key}{/if}
	</Sheet.Content>
</Sheet.Root>
