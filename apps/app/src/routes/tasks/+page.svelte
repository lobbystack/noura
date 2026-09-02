<script lang="ts">
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import type { CoreEvent, Project, Task } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Plus from 'phosphor-svelte/lib/Plus';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
	import { filterTasks, isDone, type TaskView } from '$lib/tasks/filters';
	import { tabsStore } from '$lib/tabs.svelte';

	let tasks = $state<Task[]>([]);
	let projects = $state<Project[]>([]);
	let loading = $state(true);
	let view = $state<TaskView>({ mode: 'today' });
	let selectedId = $state<string | null>(null);
	const requestedTaskId = page.url.searchParams.get('selected');
	let selected = $derived(tasks.find((task) => task.id === selectedId) ?? null);

	let projectsById = $derived(
		Object.fromEntries(projects.map((project) => [project.id, project])),
	);

	let folderGroups = $derived.by(() => {
		const groups: Record<string, number> = {};
		for (const task of tasks) {
			const parts = task.relativePath.split('/');
			const folder = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
			groups[folder] = (groups[folder] ?? 0) + 1;
		}
		return Object.entries(groups).sort(([left], [right]) =>
			left.localeCompare(right),
		);
	});

	let visibleTasks = $derived(filterTasks(tasks, view, new Date()));

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
	}

	async function load() {
		try {
			loading = true;
			const [taskList, projectList] = await Promise.all([
				getNouraClient().tasks.list(),
				getNouraClient().projects.list(),
			]);
			tasks = taskList;
			projects = projectList;
			if (!selectedId && requestedTaskId) {
				const requested = taskList.find((task) => task.id === requestedTaskId);
				if (requested) adoptSelection(requested);
			}
		} finally {
			loading = false;
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
		await load();
	}

	async function create() {
		if (!(await flushPendingDrafts())) return;
		const result = await getNouraClient().tasks.create({ title: 'New task' });
		await load();
		const created = result.value as Task;
		view = { mode: 'all' };
		await select(created);
	}

	async function handleExternalEvent(event: CoreEvent) {
		if (
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			!['object:updated', 'object:moved', 'object:deleted'].includes(event.type)
		)
			return;
		const payload = event.payload as { id?: string };
		if (event.type === 'object:deleted' && payload.id === selectedId) return;
		await load();
	}

	onMount(() => {
		void load();
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		if (browser) {
			void getNouraClient()
				.events.subscribe((event) => void handleExternalEvent(event))
				.then((unlisten) => {
					if (disposed) unlisten();
					else unsubscribe = unlisten;
				});
		}
		return () => {
			disposed = true;
			unsubscribe?.();
		};
	});
</script>

<div class="flex min-h-0 flex-1">
	<aside
		class="flex w-64 shrink-0 flex-col border-r border-border bg-background"
	>
		<header class="flex min-h-16 items-center justify-between px-4">
			<div>
				<h1 class="text-lg font-semibold">Tasks</h1>
				<p class="text-xs text-muted-foreground">
					{viewLabel(view)} · {visibleTasks.length} shown
				</p>
			</div>
			<Button
				size="icon-sm"
				onclick={() => void create()}
				aria-label="Add task"
			>
				<Plus />
			</Button>
		</header>
		<div class="min-h-0 flex-1 overflow-y-auto px-3 pb-4">
			<nav class="flex flex-col text-sm" aria-label="Task views">
				{#each ['today', 'upcoming', 'all', 'completed'] as mode (mode)}
					<button
						class="rounded-md px-3 py-2 text-left hover:bg-muted/60 {view.mode ===
						mode
							? 'bg-muted font-medium'
							: ''}"
						onclick={() => (view = { mode: mode as TaskView['mode'] })}
					>
						{mode.charAt(0).toUpperCase() + mode.slice(1)}
					</button>
				{/each}
			</nav>
			<Separator class="my-3" />
			<h2
				class="px-3 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
			>
				Projects
			</h2>
			<nav class="mt-1 flex flex-col text-sm" aria-label="Projects">
				{#each projects as project (project.id)}
					<button
						class="truncate rounded-md px-3 py-2 text-left hover:bg-muted/60"
						onclick={() => (view = { mode: 'all' })}
					>
						{project.title}
					</button>
				{:else}
					<p class="px-3 py-2 text-xs text-muted-foreground">No projects</p>
				{/each}
			</nav>
			<Separator class="my-3" />
			<h2
				class="px-3 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
			>
				Folders
			</h2>
			<nav class="mt-1 flex flex-col text-sm" aria-label="Task folders">
				{#each folderGroups as [folder, count] (folder)}
					<button
						class="flex items-center justify-between gap-2 rounded-md px-3 py-2 text-left hover:bg-muted/60 {view.mode ===
							'folder' && view.folderPath === folder
							? 'bg-muted font-medium'
							: ''}"
						onclick={() => (view = { mode: 'folder', folderPath: folder })}
					>
						<span class="truncate">{folder || '/'}</span>
						<span class="text-xs text-muted-foreground">{count}</span>
					</button>
				{/each}
			</nav>
		</div>
	</aside>

	<section class="flex w-96 shrink-0 flex-col border-r border-border">
		{#if loading}
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
												class="text-[10px] uppercase"
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
				<TaskDetail task={selected} {projects} onupdated={() => void load()} />
			{/key}
		{/if}
	</main>
</div>
