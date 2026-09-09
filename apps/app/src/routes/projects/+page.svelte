<script lang="ts">
	import { getRouteSidebar } from '$lib/route-sidebar.svelte';
	const routeSidebar = getRouteSidebar();
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	import { afterNavigate, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import type {
		CalendarEntry,
		KanbanGroup,
		Note,
		Project,
		Task,
		WorkspaceEntry,
	} from '@noura/workspace';
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { LiveProjection } from '$lib/live-refresh';
	import { formatCalendarBoundary } from '$lib/calendar';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import ProjectOverview from '$lib/components/project-overview.svelte';
	import ProjectBoard from '$lib/components/project-board.svelte';
	import TaskDetail from '$lib/components/task-detail.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';
	import Trash from 'phosphor-svelte/lib/Trash';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';

	type Summary = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['projects']['listSummaries']>
	>[number];
	let summaries = $state<Summary[]>([]);
	let selectedId = $state<string | null>(null);
	let selectedProjectSnapshot = $state.raw<Project | null>(null);
	let selected = $derived(
		summaries.find((entry) => entry.project.id === selectedId)?.project ??
			(selectedProjectSnapshot?.id === selectedId
				? selectedProjectSnapshot
				: null),
	);
	let tasks = $state<Task[]>([]);
	let notes = $state<Note[]>([]);
	let files = $state<WorkspaceEntry[]>([]);
	let calendar = $state<CalendarEntry[]>([]);
	let boardGroups = $state<KanbanGroup[]>([]);
	let selectedTask = $state<Task | null>(null);
	let selectedNote = $state<Note | null>(null);
	let loading = $state(true);
	let tab = $state('overview');
	let deleteOpen = $state(false);
	let projectLoadGeneration = 0;
	let requestedProjectGeneration = 0;
	const requestedProjectId = $derived(page.url.searchParams.get('selected'));
	let liveRefresh = $state.raw<LiveProjection | null>(null);

	function monthRange() {
		const now = new Date();
		const start = new Date(now.getFullYear(), now.getMonth(), 1);
		const end = new Date(now.getFullYear(), now.getMonth() + 1, 1);
		return {
			start: formatCalendarBoundary(start),
			end: formatCalendarBoundary(end),
		};
	}

	async function loadProjects() {
		const nextSummaries = await getNouraClient().projects.listSummaries();
		const currentSelection = selected;
		const currentSelectedId = selectedId;
		const selectedStillExists =
			currentSelectedId !== null &&
			nextSummaries.some((entry) => entry.project.id === currentSelectedId);
		const retainDeletedSelection =
			currentSelectedId !== null &&
			!selectedStillExists &&
			currentSelection?.id === currentSelectedId;

		// An external deletion must not unmount ProjectOverview before its
		// coordinator can surface and resolve the preserved draft.
		selectedProjectSnapshot = retainDeletedSelection ? currentSelection : null;
		summaries = nextSummaries;
		if (!currentSelectedId) {
			selectedId =
				summaries.find((entry) => entry.project.id === requestedProjectId)
					?.project.id ??
				summaries[0]?.project.id ??
				null;
			return;
		}
		if (!selectedStillExists && !retainDeletedSelection)
			selectedId = summaries[0]?.project.id ?? null;
	}

	async function loadProject() {
		const generation = ++projectLoadGeneration;
		const projectId = selectedId;
		if (!projectId) {
			tasks = [];
			notes = [];
			files = [];
			calendar = [];
			boardGroups = [];
			selectedTask = null;
			selectedNote = null;
			return;
		}
		const range = monthRange();
		const loaded = await Promise.all([
			getNouraClient().projects.listTasks({ projectId }),
			getNouraClient().projects.listFolderNotes({ projectId }),
			getNouraClient().projects.listFolderFiles({ projectId }),
			getNouraClient().projects.queryCalendar({
				projectId,
				...range,
			}),
			getNouraClient().kanban.getBoard({ projectId }),
		]);
		if (generation !== projectLoadGeneration || selectedId !== projectId)
			return;
		[tasks, notes, files, calendar, { groups: boardGroups }] = loaded;
		if (selectedTask) {
			// Retain a deleted selection until TaskDetail handles its external-delete
			// conflict flow; clearing it here would discard the protected draft UI.
			const current = tasks.find((task) => task.id === selectedTask?.id);
			if (current) selectedTask = current;
		}
		if (selectedNote) {
			const current = notes.find((note) => note.id === selectedNote?.id);
			if (current) selectedNote = current;
		}
	}

	async function load() {
		const isInitialLoad = loading;
		try {
			await loadProjects();
			// Folder projections resolve the project object, which no longer exists
			// while ProjectOverview is preserving an external-delete draft.
			if (selectedProjectSnapshot?.id === selectedId) return;
			await loadProject();
		} finally {
			if (isInitialLoad) loading = false;
		}
	}

	async function refreshProjection() {
		if (liveRefresh) await liveRefresh.refreshNow();
		else await load();
	}

	function replaceSelectedProjectInUrl(projectId: string | null) {
		const next = new URL(page.url);
		if (projectId) next.searchParams.set('selected', projectId);
		else next.searchParams.delete('selected');
		replaceState(next, {});
	}

	async function select(
		project: Project,
		options?: { requestedGeneration?: number },
	) {
		if (project.id !== selectedId && !(await flushPendingDrafts())) return;
		if (
			options?.requestedGeneration !== undefined &&
			options.requestedGeneration !== requestedProjectGeneration
		)
			return;
		selectedId = project.id;
		selectedTask = null;
		selectedNote = null;
		tabsStore.open(project.id, 'project', project.title);
		// Keep the URL authoritative without adding a history entry for every
		// project click. Otherwise the deep-link effect can restore a stale ID.
		if (page.url.searchParams.get('selected') !== project.id) {
			replaceSelectedProjectInUrl(project.id);
		}
		await loadProject();
	}

	async function create() {
		if (!(await flushPendingDrafts())) return;
		const result = await getNouraClient().projects.create({
			title: 'New project',
			properties: { status: 'planned' },
		});
		await loadProjects();
		await select(result.value as Project);
	}

	// Search navigation may update only the query string while this page stays
	// mounted. Keep the selection synchronized after every navigation settles.
	afterNavigate(() => {
		const generation = ++requestedProjectGeneration;
		const requestedId = requestedProjectId;
		if (!browser || loading || !requestedId || requestedId === selectedId)
			return;
		void (async () => {
			// Search and external filesystem changes can discover a project after
			// this page's lookup projection was loaded. Refresh before resolving it.
			await loadProjects();
			if (
				generation !== requestedProjectGeneration ||
				requestedProjectId !== requestedId ||
				requestedId === selectedId
			)
				return;
			const requested = summaries.find(
				(entry) => entry.project.id === requestedId,
			);
			if (requested) {
				await select(requested.project, { requestedGeneration: generation });
			}
		})();
	});

	async function createTask() {
		if (!selected || !(await flushPendingDrafts())) return;
		const result = await getNouraClient().tasks.create({
			title: 'New task',
			properties: { project: selected.id, status: 'todo', priority: 'medium' },
		});
		await refreshProjection();
		selectedTask = result.value as Task;
		tabsStore.open(selectedTask.id, 'task', selectedTask.title);
	}

	async function openTask(task: Task) {
		if (selectedTask?.id !== task.id && !(await flushPendingDrafts())) return;
		selectedTask = task;
		tabsStore.open(task.id, 'task', task.title);
	}

	async function openNote(note: Note) {
		if (selectedNote?.id !== note.id && !(await flushPendingDrafts())) return;
		selectedNote = note;
		tabsStore.open(note.id, 'note', note.title);
	}

	function handleNoteSaved(updated: Note) {
		notes = notes.map((note) => (note.id === updated.id ? updated : note));
		selectedNote = updated;
		tabsStore.renameObject(updated.id, updated.title);
	}

	async function removeProject() {
		if (!selected || !(await flushPendingDrafts())) return;
		await getNouraClient().projects.delete({
			id: selected.id,
			expectedRevision: selected.revision,
		});
		deleteOpen = false;
		selectedId = null;
		replaceSelectedProjectInUrl(null);
		await refreshProjection();
		replaceSelectedProjectInUrl(selectedId);
	}

	function statusVariant(status: unknown): 'default' | 'secondary' | 'outline' {
		if (status === 'active') return 'default';
		if (status === 'on-hold') return 'outline';
		return 'secondary';
	}

	onMount(() => {
		if (!browser) return;
		const coordinator = new LiveProjection({
			refresh: load,
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
		});
		liveRefresh = coordinator;
		void coordinator.start().catch(() => {});
		return () => {
			coordinator.dispose();
			if (liveRefresh === coordinator) liveRefresh = null;
		};
	});
</script>

{#snippet projectSidebar()}
	<Sidebar.SidebarGroup>
		<Sidebar.SidebarGroupLabel
			>Projects <span class="ml-2 font-normal">{summaries.length}</span
			></Sidebar.SidebarGroupLabel
		>
		<Sidebar.SidebarGroupAction
			onclick={() => void create()}
			aria-label="New project"
			title="New project"><Plus /></Sidebar.SidebarGroupAction
		>
		<Sidebar.SidebarGroupContent>
			{#if loading}
				<div class="flex flex-col gap-1">
					{#each [0, 1, 2] as item (item)}<Skeleton
							class="h-12 w-full"
						/>{/each}
				</div>
			{:else if summaries.length === 0}
				<EmptyState
					icon={FolderOpen}
					title="No projects"
					description="Organize work with projects."
					actionLabel="New project"
					onAction={create}
				/>
			{:else}
				<nav aria-label="Projects">
					<Sidebar.SidebarMenu>
						{#each summaries as summary (summary.project.id)}
							<Sidebar.SidebarMenuItem>
								<Sidebar.SidebarMenuButton
									size="lg"
									isActive={selectedId === summary.project.id}
									onclick={() => void select(summary.project)}
									title={summary.project.title}
								>
									<div class="min-w-0 flex-1">
										<span class="block truncate text-xs font-medium"
											>{summary.project.title}</span
										>
										<span class="text-xs text-muted-foreground"
											>{summary.taskCount} task{summary.taskCount === 1
												? ''
												: 's'}</span
										>
									</div>
									<Badge
										class="max-w-20 shrink-0 truncate"
										variant={statusVariant(summary.project.properties.status)}
										>{String(
											summary.project.properties.status ?? 'planned',
										)}</Badge
									>
								</Sidebar.SidebarMenuButton>
							</Sidebar.SidebarMenuItem>
						{/each}
					</Sidebar.SidebarMenu>
				</nav>
			{/if}
		</Sidebar.SidebarGroupContent>
	</Sidebar.SidebarGroup>
{/snippet}

<div
	class="flex min-h-0 flex-1"
	{@attach () => routeSidebar.mount(projectSidebar)}
>
	<main class="flex min-w-0 flex-1 flex-col">
		{#if !selected}<EmptyState
				icon={FolderOpen}
				title="Select a project"
				description="Choose a project or create a new one."
			/>
		{:else}
			<header class="flex min-h-16 items-center gap-3 px-6">
				<div class="min-w-0 flex-1">
					<h1 class="truncate text-lg font-semibold">{selected.title}</h1>
					<p class="truncate text-xs text-muted-foreground">
						{selected.relativePath}
					</p>
				</div>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => (deleteOpen = true)}
					aria-label="Delete project"><Trash /></Button
				>
			</header>
			<Separator />
			<Tabs.Root bind:value={tab} class="flex min-h-0 flex-1 flex-col">
				<Tabs.List class="mx-6 mt-3 w-fit"
					><Tabs.Trigger value="overview">Overview</Tabs.Trigger><Tabs.Trigger
						value="tasks">Tasks</Tabs.Trigger
					><Tabs.Trigger value="board">Board</Tabs.Trigger><Tabs.Trigger
						value="calendar">Calendar</Tabs.Trigger
					><Tabs.Trigger value="notes">Notes</Tabs.Trigger><Tabs.Trigger
						value="files">Files</Tabs.Trigger
					></Tabs.List
				>
				<Tabs.Content value="overview" class="flex min-h-0 flex-1"
					>{#key selected.id}<ProjectOverview
							project={selected}
							onupdated={() => void refreshProjection()}
						/>{/key}</Tabs.Content
				>
				<Tabs.Content value="tasks" class="flex min-h-0 flex-1"
					><section class="flex w-80 shrink-0 flex-col border-r border-border">
						<div class="flex items-center justify-between px-4 py-3">
							<span class="text-sm font-medium">Project tasks</span><Button
								size="sm"
								onclick={() => void createTask()}
								><Plus data-icon="inline-start" />Add task</Button
							>
						</div>
						<Separator />
						<div
							class="min-h-0 flex-1 overflow-y-auto divide-y divide-border/60"
						>
							{#each tasks as task (task.id)}<button
									class="w-full px-4 py-3 text-left hover:bg-muted/50 {selectedTask?.id ===
									task.id
										? 'bg-muted'
										: ''}"
									onclick={() => void openTask(task)}
									><span class="block truncate text-sm font-medium"
										>{task.title}</span
									><span class="text-xs text-muted-foreground"
										>{String(task.properties.status ?? 'todo')}</span
									></button
								>{:else}<p class="p-4 text-sm text-muted-foreground">
									No tasks in this project.
								</p>{/each}
						</div>
					</section>
					<section class="flex min-w-0 flex-1">
						{#if selectedTask}{#key selectedTask.id}<TaskDetail
									task={selectedTask}
									projects={summaries.map((entry) => entry.project)}
									onupdated={() => void loadProject()}
								/>{/key}{:else}<EmptyState
								icon={FolderOpen}
								title="Select a task"
								description="Choose a project task to edit it."
							/>{/if}
					</section></Tabs.Content
				>
				<Tabs.Content value="board" class="flex min-h-0 flex-1"
					>{#key selected.id}<ProjectBoard
							projectId={selected.id}
							projectTitle={selected.title}
							groups={boardGroups}
							projects={summaries.map((entry) => entry.project)}
							onRefresh={refreshProjection}
						/>{/key}</Tabs.Content
				>
				<Tabs.Content value="calendar" class="min-h-0 flex-1 overflow-y-auto"
					><div class="divide-y divide-border/60">
						{#each calendar as entry (`${entry.sourceId}:${entry.property}`)}<button
								class="flex w-full items-center gap-4 px-6 py-3 text-left hover:bg-muted/50"
								onclick={() => {
									const task = tasks.find(
										(candidate) => candidate.id === entry.sourceId,
									);
									if (task) {
										tab = 'tasks';
										void openTask(task);
									}
								}}
								><time class="w-32 text-xs text-muted-foreground"
									>{entry.start}</time
								><span class="text-sm font-medium">{entry.title}</span><Badge
									variant="secondary">{entry.property}</Badge
								></button
							>{:else}<p class="p-6 text-sm text-muted-foreground">
								No dated project work this month.
							</p>{/each}
					</div></Tabs.Content
				>
				<Tabs.Content value="notes" class="flex min-h-0 flex-1"
					><section class="flex w-80 shrink-0 flex-col border-r border-border">
						<div class="px-4 py-3 text-sm font-medium">Project notes</div>
						<Separator />
						<div
							class="min-h-0 flex-1 overflow-y-auto divide-y divide-border/60"
						>
							{#each notes as note (note.id)}<button
									class="w-full px-4 py-3 text-left hover:bg-muted/50 {selectedNote?.id ===
									note.id
										? 'bg-muted'
										: ''}"
									onclick={() => void openNote(note)}
									><span class="block truncate text-sm font-medium"
										>{note.title}</span
									><span class="block truncate text-xs text-muted-foreground"
										>{note.relativePath}</span
									></button
								>{:else}<p class="p-4 text-sm text-muted-foreground">
									No managed notes are in this project folder.
								</p>{/each}
						</div>
					</section>
					<section class="flex min-w-0 flex-1">
						{#if selectedNote}{#key selectedNote.id}<NoteEditor
									note={selectedNote}
									onsaved={handleNoteSaved}
								/>{/key}{:else}<EmptyState
								icon={NotePencil}
								title="Select a note"
								description="Choose a project note to edit it."
							/>{/if}
					</section></Tabs.Content
				>
				<Tabs.Content value="files" class="min-h-0 flex-1 overflow-y-auto"
					><div class="divide-y divide-border/60">
						{#each files as file (file.relativePath)}<div
								class="flex items-center gap-3 px-6 py-3"
							>
								<span class="min-w-0 flex-1 truncate text-sm">{file.name}</span
								><span class="truncate text-xs text-muted-foreground"
									>In project folder · {file.relativePath}</span
								>
							</div>{:else}<p class="p-6 text-sm text-muted-foreground">
								This project folder contains no files.
							</p>{/each}
					</div></Tabs.Content
				>
			</Tabs.Root>
		{/if}
	</main>
</div>

<AlertDialog.Root bind:open={deleteOpen}
	><AlertDialog.Content
		><AlertDialog.Header
			><AlertDialog.Title>Delete this project?</AlertDialog.Title
			><AlertDialog.Description
				>Noura moves only the project file to trash. Tasks linked to this
				project remain in the workspace.</AlertDialog.Description
			></AlertDialog.Header
		><AlertDialog.Footer
			><AlertDialog.Cancel>Cancel</AlertDialog.Cancel><AlertDialog.Action
				onclick={() => void removeProject()}>Delete project</AlertDialog.Action
			></AlertDialog.Footer
		></AlertDialog.Content
	></AlertDialog.Root
>
