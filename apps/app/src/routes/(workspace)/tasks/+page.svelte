<script lang="ts">
	import { goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import type { Task } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { tasksData } from '$lib/tasks-data.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import { dueLabel, isOverdue } from '$lib/dashboard-dates';
	import { cn } from '$lib/utils';
	import EmptyState from '$lib/components/empty-state.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { filterTasks, isDone, type TaskView } from '$lib/tasks/filters';
	import { tabsStore } from '$lib/tabs.svelte';
	import TaskNameInput from '$lib/components/task-name-input.svelte';
	import * as ContextMenu from '$lib/components/ui/context-menu/index.js';
	import { createTask, renameTask, trashTask } from '$lib/task-actions';

	let selectedId = $state<string | null>(null);
	let loading = $state(true);
	/** A new task is being named; it exists only once named. */
	let drafting = $state(false);
	let renamingId = $state<string | null>(null);

	// The view lives in the URL so the per-module sidebar section drives it
	// and views stay linkable: /tasks?view=today, ?view=folder&folder=notes,
	// ?view=project&project=<id>.
	const view = $derived.by((): TaskView => {
		const params = page.url.searchParams;
		const mode = params.get('view') ?? 'today';
		if (mode === 'folder')
			return { mode: 'folder', folderPath: params.get('folder') ?? '' };
		if (mode === 'project')
			return { mode: 'project', projectId: params.get('project') ?? '' };
		if (mode === 'upcoming' || mode === 'all' || mode === 'completed')
			return { mode };
		return { mode: 'today' };
	});

	const projectsById = $derived(
		new Map(tasksData.projects.map((project) => [project.id, project])),
	);

	const visibleTasks = $derived(filterTasks(tasksData.tasks, view, new Date()));
	const selected = $derived(
		tasksData.tasks.find((task) => task.id === selectedId) ?? null,
	);

	const viewLabel = $derived.by(() => {
		if (view.mode === 'folder') return view.folderPath || 'Top level';
		if (view.mode === 'project')
			return projectsById.get(view.projectId ?? '')?.title ?? 'Project';
		return view.mode.charAt(0).toUpperCase() + view.mode.slice(1);
	});

	function projectLabel(task: Task): string {
		const projectId =
			typeof task.properties?.project === 'string'
				? task.properties.project
				: '';
		if (!projectId || view.mode === 'project') return '';
		return projectsById.get(projectId)?.title ?? '';
	}

	function adoptSelection(task: Task) {
		selectedId = task.id;
		tabsStore.open(task.id, 'task', task.title);
		// Keep the URL truthful: the deep-link effect re-runs on every tasks
		// reload, and a stale ?selected= param would yank the selection back.
		// replaceState avoids a history entry per click.
		if (page.url.searchParams.get('selected') !== task.id) {
			const next = new URL(page.url);
			next.searchParams.set('selected', task.id);
			replaceState(next, {});
		}
	}

	async function select(task: Task) {
		if (selectedId === task.id) return;
		if (!(await flushPendingDrafts())) return;
		adoptSelection(task);
	}

	async function toggleDone(task: Task) {
		const done = isDone(task);
		try {
			await getNouraClient().tasks[done ? 'reopen' : 'complete']({
				id: task.id,
				expectedRevision: task.revision ?? '',
			});
		} catch {
			toast.error('Could not update the task', {
				description: 'It changed elsewhere. Showing the latest version.',
			});
		}
		await tasksData.load();
	}

	async function create(title: string): Promise<boolean> {
		if (!(await flushPendingDrafts())) return false;
		const properties =
			view.mode === 'project' && view.projectId
				? { project: view.projectId }
				: undefined;
		const created = await createTask(title, properties);
		if (!created) return false;
		drafting = false;
		await tasksData.load();
		// Show the new task wherever its due date would hide it.
		if (view.mode !== 'all' && view.mode !== 'project') {
			await goto('/tasks?view=all');
		}
		adoptSelection(created);
		return true;
	}

	async function rename(task: Task, title: string): Promise<boolean> {
		const renamed = await renameTask(task, title);
		// A failed rename keeps the field open so the name can be saved again.
		if (!renamed) return false;
		renamingId = null;
		await tasksData.load();
		return true;
	}

	async function openInNewTab(task: Task) {
		tabsStore.keepObject(task.id);
		await select(task);
	}

	function deselect() {
		selectedId = null;
		if (page.url.searchParams.has('selected')) {
			const next = new URL(page.url);
			next.searchParams.delete('selected');
			replaceState(next, {});
		}
	}

	async function trash(task: Task) {
		// Save the open task first, then close it, so it doesn't report its
		// own file as deleted.
		await flushPendingDrafts();
		if (selectedId === task.id) deselect();
		if (await trashTask(task)) await tasksData.load();
	}

	// Tree and sidebar deep links land on /tasks?selected=<id>.
	$effect(() => {
		const requested = page.url.searchParams.get('selected');
		if (!requested || requested === selectedId) return;
		const requestedTask = tasksData.tasks.find((task) => task.id === requested);
		if (requestedTask) adoptSelection(requestedTask);
	});

	onMount(() => {
		void tasksData.start().finally(() => {
			loading = false;
		});
	});
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<PageHeader title="Tasks" description={viewLabel}>
		{#snippet actions()}
			<Button size="sm" onclick={() => (drafting = true)}>
				<Plus data-icon="inline-start" />
				New task
			</Button>
		{/snippet}
	</PageHeader>

	<div class="flex min-h-0 flex-1">
		<section
			class="flex w-96 shrink-0 flex-col border-r border-border"
			aria-label="Task list"
		>
			{#if loading && tasksData.tasks.length === 0}
				<div class="flex flex-col gap-2 p-4" aria-hidden="true">
					{#each [0, 1, 2, 3] as row (row)}
						<Skeleton class="h-10 w-full" />
					{/each}
				</div>
			{:else if visibleTasks.length === 0 && !drafting}
				<EmptyState icon={Checks} title="No tasks here" />
			{:else}
				{#if drafting}
					<div
						class="flex items-center gap-3 border-b border-border/60 px-4 py-2"
					>
						<Checkbox disabled aria-hidden="true" tabindex={-1} />
						<TaskNameInput
							label="New task name"
							oncommit={create}
							oncancel={() => (drafting = false)}
						/>
					</div>
				{/if}
				<ul class="min-h-0 flex-1 divide-y divide-border/60 overflow-y-auto">
					{#each visibleTasks as task (task.id)}
						{@const done = isDone(task)}
						{@const due =
							typeof task.properties?.due === 'string'
								? task.properties.due
								: ''}
						{@const priority = task.properties?.priority}
						{@const project = projectLabel(task)}
						<ContextMenu.Root>
							<ContextMenu.Trigger>
								{#snippet child({ props })}
									<li
										{...props}
										class={cn(
											'flex items-center gap-3 px-4 hover:bg-muted/40',
											selectedId === task.id && 'bg-muted/60',
										)}
									>
										<Checkbox
											checked={done}
											onCheckedChange={() => void toggleDone(task)}
											aria-label={done
												? `Mark ${task.title} as not done`
												: `Mark ${task.title} as done`}
										/>
										{#if renamingId === task.id}
											<div class="min-w-0 flex-1 py-2">
												<TaskNameInput
													value={task.title}
													label="New name for {task.title}"
													oncommit={(title) => rename(task, title)}
													oncancel={() => (renamingId = null)}
												/>
											</div>
										{:else}
											<button
												type="button"
												class="min-w-0 flex-1 py-3 text-left"
												aria-current={selectedId === task.id
													? 'true'
													: undefined}
												onclick={() => select(task)}
											>
												<span
													class={cn(
														'block truncate text-sm font-medium',
														done && 'text-muted-foreground line-through',
													)}>{task.title}</span
												>
												{#if project || due || (priority && priority !== 'medium')}
													<span
														class="mt-0.5 flex items-center gap-2 text-xs text-muted-foreground"
													>
														{#if project}<span class="truncate">{project}</span
															>{/if}
														{#if due}<span
																class={cn(
																	'shrink-0',
																	!done &&
																		isOverdue(due, new Date()) &&
																		'text-destructive',
																)}>{dueLabel(due, new Date())}</span
															>{/if}
														{#if priority && priority !== 'medium'}<Badge
																variant="secondary"
																class="capitalize">{priority}</Badge
															>{/if}
													</span>
												{/if}
											</button>
										{/if}
									</li>
								{/snippet}
							</ContextMenu.Trigger>
							<ContextMenu.Content class="w-52">
								<ContextMenu.Group>
									<ContextMenu.Item onclick={() => void select(task)}>
										Open
									</ContextMenu.Item>
									<ContextMenu.Item onclick={() => void openInNewTab(task)}>
										Open in new tab
									</ContextMenu.Item>
								</ContextMenu.Group>
								<ContextMenu.Separator />
								<ContextMenu.Group>
									<ContextMenu.Item onclick={() => (renamingId = task.id)}>
										Rename…
									</ContextMenu.Item>
									<ContextMenu.Item onclick={() => void toggleDone(task)}>
										{done ? 'Mark as not done' : 'Mark as done'}
									</ContextMenu.Item>
								</ContextMenu.Group>
								<ContextMenu.Separator />
								<ContextMenu.Item
									variant="destructive"
									onclick={() => void trash(task)}
								>
									Move to Trash
								</ContextMenu.Item>
							</ContextMenu.Content>
						</ContextMenu.Root>
					{/each}
				</ul>
			{/if}
		</section>

		<div class="flex min-w-0 flex-1 flex-col">
			{#if !selected}
				<EmptyState icon={Checks} title="Select a task" />
			{:else}
				{#key selected.id}
					<TaskDetail
						task={selected}
						projects={tasksData.projects}
						onupdated={() => void tasksData.load()}
					/>
				{/key}
			{/if}
		</div>
	</div>
</div>
