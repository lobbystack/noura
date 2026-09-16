<script lang="ts">
	import { browser } from '$app/environment';
	import { goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import type { Task } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { tasksData } from '$lib/tasks-data.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Plus from 'phosphor-svelte/lib/Plus';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
	import { filterTasks, isDone, type TaskView } from '$lib/tasks/filters';
	import { tabsStore } from '$lib/tabs.svelte';

	let selectedId = $state<string | null>(null);
	let loading = $state(true);

	// The view lives in the URL so the per-module sidebar section drives it
	// and views stay linkable: /tasks?view=today, ?view=folder&folder=notes.
	const view = $derived.by((): TaskView => {
		const mode = page.url.searchParams.get('view') ?? 'today';
		if (mode === 'folder') {
			return {
				mode: 'folder',
				folderPath: page.url.searchParams.get('folder') ?? '',
			};
		}
		if (mode === 'upcoming' || mode === 'all' || mode === 'completed') {
			return { mode };
		}
		return { mode: 'today' };
	});

	const projectsById = $derived(
		Object.fromEntries(
			tasksData.projects.map((project) => [project.id, project]),
		),
	);

	const visibleTasks = $derived(filterTasks(tasksData.tasks, view, new Date()));
	const selected = $derived(
		tasksData.tasks.find((task) => task.id === selectedId) ?? null,
	);

	function viewLabel(current: TaskView): string {
		if (current.mode === 'folder') return current.folderPath ?? 'Folder';
		return current.mode.charAt(0).toUpperCase() + current.mode.slice(1);
	}

	function projectLabel(
		properties: Record<string, unknown> | undefined,
	): string {
		const projectId =
			typeof properties?.project === 'string' ? properties.project : '';
		if (!projectId) return '';
		return projectsById[projectId]?.title ?? projectId;
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
		await getNouraClient().tasks[done ? 'reopen' : 'complete']({
			id: task.id,
			expectedRevision: task.revision ?? '',
		});
		await tasksData.load();
	}

	async function create() {
		if (!(await flushPendingDrafts())) return;
		const result = await getNouraClient().tasks.create({ title: 'New task' });
		await tasksData.load();
		const created = result.value as Task;
		// Move to All so the fresh task is visible regardless of its due date.
		if (view.mode !== 'all') {
			await goto('/tasks?view=all');
		}
		adoptSelection(created);
	}

	// Tree and sidebar deep links land on /tasks?selected=<id>.
	$effect(() => {
		if (!browser) return;
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

<div class="flex h-full flex-col">
	<div
		class="flex h-14 shrink-0 items-center justify-between border-b border-border px-6"
	>
		<div class="min-w-0">
			<h1 class="text-sm font-semibold">Tasks</h1>
			<p class="truncate text-xs text-muted-foreground">
				{viewLabel(view)} · {visibleTasks.length} shown
			</p>
		</div>
		<Button size="sm" onclick={() => void create()}>
			<Plus data-icon="inline-start" />
			New task
		</Button>
	</div>

	<div class="flex min-h-0 flex-1">
		<section class="flex w-96 shrink-0 flex-col border-r border-border">
			{#if loading && tasksData.tasks.length === 0}
				<div class="flex-1 animate-pulse bg-muted/40" aria-hidden="true"></div>
			{:else if visibleTasks.length === 0}
				<div
					class="flex flex-1 items-center justify-center p-6 text-sm text-muted-foreground"
				>
					Nothing here.
				</div>
			{:else}
				<div class="min-h-0 flex-1 overflow-y-auto">
					<div class="divide-y divide-border/60">
						{#each visibleTasks as task (task.id)}
							{@const done = isDone(task)}
							<button
								class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-muted/40 {selectedId ===
								task.id
									? 'bg-muted/60'
									: ''}"
								onclick={() => select(task)}
							>
								<input
									type="checkbox"
									checked={done}
									class="size-4 rounded border-input"
									onclick={(event) => {
										event.stopPropagation();
										void toggleDone(task);
									}}
									aria-label="Toggle {task.title}"
								/>
								<div class="min-w-0 flex-1">
									<span
										class="block truncate text-sm font-medium {done
											? 'text-muted-foreground line-through'
											: ''}">{task.title}</span
									>
									{#if projectLabel(task.properties) || task.properties?.due || (task.properties?.priority && task.properties.priority !== 'medium')}
										<div
											class="mt-0.5 flex items-center gap-2 text-xs text-muted-foreground"
										>
											{#if projectLabel(task.properties)}<span class="truncate"
													>{projectLabel(task.properties)}</span
												>{/if}
											{#if task.properties?.due}<CalendarBlank
													class="size-3 shrink-0"
												/><span>{task.properties.due}</span>{/if}
											{#if task.properties?.priority && task.properties.priority !== 'medium'}<Badge
													variant="secondary"
													class="text-xs uppercase"
													>{task.properties.priority}</Badge
												>{/if}
										</div>
									{/if}
								</div>
							</button>
						{/each}
					</div>
				</div>
			{/if}
		</section>

		<main class="flex min-w-0 flex-1 flex-col">
			{#if !selected}
				<EmptyState
					icon={NotePencil}
					title="Select a task"
					description="Choose a task from the list or create a new one."
				/>
			{:else}
				{#key selected.id}
					<TaskDetail
						task={selected}
						projects={tasksData.projects}
						onupdated={() => void tasksData.load()}
					/>
				{/key}
			{/if}
		</main>
	</div>
</div>
