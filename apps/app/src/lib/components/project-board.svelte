<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { toast } from 'svelte-sonner';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import type { KanbanGroup, Project, Task } from '@noura/workspace';
	import { cn } from '$lib/utils';
	import { dueLabel } from '$lib/dashboard-dates';
	import DotsThree from 'phosphor-svelte/lib/DotsThree';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import X from 'phosphor-svelte/lib/X';

	let { projectId, projectTitle, groups, projects, onRefresh } = $props<{
		projectId: string;
		projectTitle: string;
		groups: KanbanGroup[];
		projects: Project[];
		onRefresh: () => Promise<void>;
	}>();

	let selectedTaskId = $state<string | null>(null);
	/** The task as last seen. It stays shown if the task disappears, since
	 * TaskDetail owns the external-delete flow and may protect a draft. */
	let selectedSnapshot = $state.raw<Task | null>(null);
	const selectedTask = $derived(
		(selectedTaskId &&
			groups
				.flatMap((group: KanbanGroup) => group.items)
				.find((candidate: Task) => candidate.id === selectedTaskId)) ||
			selectedSnapshot,
	);
	let detailOpen = $state(false);
	let closingDetail = $state(false);
	let dragTask = $state<Task | null>(null);
	let dragOverColumn = $state<string | null>(null);
	let dragOverTaskId = $state<string | null>(null);

	const columnLabel = (id: string) =>
		id
			.split('-')
			.map((part) => part.charAt(0).toUpperCase() + part.slice(1))
			.join(' ');

	const isEmpty = $derived(
		groups.every((g: KanbanGroup) => g.items.length === 0),
	);

	function select(task: Task) {
		selectedTaskId = task.id;
		selectedSnapshot = task;
		detailOpen = true;
		tabsStore.open(task.id, 'task', task.title);
	}

	async function create() {
		const result = await getNouraClient().tasks.create({
			title: 'New task',
			properties: { project: projectId, status: 'todo', priority: 'medium' },
		});
		await onRefresh();
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
			await onRefresh();
			toast.error('Could not move the task', {
				description:
					'The task changed while you were viewing it. Refreshed to the latest state.',
			});
			return;
		}
		await onRefresh();
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
		const group = groups.find((entry: KanbanGroup) => entry.id === columnId);
		const beforeIndex = beforeId
			? (group?.items.findIndex((item: Task) => item.id === beforeId) ?? -1)
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
			await onRefresh();
			toast.error('Could not move the task', {
				description:
					'The task changed while you were viewing it. Refreshed to the latest state.',
			});
			return;
		}
		await onRefresh();
	}
</script>

{#if isEmpty}
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
				class={cn(
					'flex w-64 shrink-0 flex-col bg-background transition-colors',
					dragOverColumn === group.id && 'bg-muted/50',
				)}
				ondragover={(event) => handleDragOver(event, group.id)}
				ondragleave={() => handleDragLeave(group.id)}
				ondrop={(event) => handleDrop(event, group.id)}
				aria-label={`${columnLabel(group.id)} column`}
			>
				<header
					class="flex items-center justify-between border-b border-border/60 px-3 py-2.5"
				>
					<h3 class="text-xs font-medium text-muted-foreground">
						{columnLabel(group.id)}
					</h3>
					<span class="text-xs text-muted-foreground">{group.items.length}</span
					>
				</header>
				<div
					class="flex flex-1 flex-col gap-px overflow-y-auto bg-border/40 p-px"
				>
					{#each group.items as task (task.id)}
						{@const done = task.properties?.status === 'done'}
						{@const priority = task.properties?.priority}
						<div
							role="listitem"
							class={cn(
								'flex items-start bg-background transition-colors hover:bg-muted/70',
								dragTask?.id === task.id && 'opacity-40',
								dragOverTaskId === task.id &&
									'ring-1 ring-inset ring-primary/40',
							)}
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
									class={cn(
										'block text-sm font-medium',
										done && 'text-muted-foreground line-through',
									)}>{task.title}</span
								>{#if task.properties?.due || (typeof priority === 'string' && priority !== 'medium')}<span
										class="mt-1 flex items-center gap-1.5"
										>{#if task.properties?.due}<span
												class="text-xs text-muted-foreground"
												>{dueLabel(
													String(task.properties.due),
													new Date(),
												)}</span
											>{/if}{#if typeof priority === 'string' && priority !== 'medium'}<Badge
												variant="secondary"
												class="capitalize">{priority}</Badge
											>{/if}</span
									>{/if}</button
							>
							<DropdownMenu.Root
								><DropdownMenu.Trigger
									>{#snippet child({ props })}<Button
											{...props}
											variant="ghost"
											size="icon-sm"
											aria-label="Move {task.title}"><DotsThree /></Button
										>{/snippet}</DropdownMenu.Trigger
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
					onupdated={() => void onRefresh()}
				/>{/key}{/if}
	</Sheet.Content>
</Sheet.Root>
