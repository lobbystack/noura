<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import {
		createLiveMarkdownDocument,
		createLiveMarkdownEditor,
		type LiveMarkdownEditor,
	} from '@noura/editor';
	import type { CoreEvent, Project, Task } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Plus from 'phosphor-svelte/lib/Plus';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
	import Warning from 'phosphor-svelte/lib/Warning';
	import { filterTasks, isDone, type TaskView } from '$lib/tasks/filters';
	import { tabsStore } from '$lib/tabs.svelte';

	type TaskDraft = {
		title: string;
		body: string;
		properties: Record<string, unknown>;
	};
	type ConflictState = { draft: TaskDraft; file: Task };
	type Resolution = 'use-external' | 'replace-external';

	const STATUSES = ['todo', 'in-progress', 'done', 'cancelled'] as const;
	const PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const;

	let tasks = $state<Task[]>([]);
	let projects = $state<Project[]>([]);
	let loading = $state(true);
	let view = $state<TaskView>({ mode: 'today' });
	let selectedId = $state<string | null>(null);
	let selected = $derived(tasks.find((task) => task.id === selectedId) ?? null);
	let draftTitle = $state('');
	let baseTitle = $state('');
	let baseBody = $state('');
	let baseRevision = $state('');
	let baseProperties = $state<Record<string, unknown>>({});
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<TaskDraft> | null>(null);
	let autosaveError = $state<unknown | null>(null);
	let transientMessage = $state<string | null>(null);
	let clearMessageTimer: ReturnType<typeof setTimeout> | null = null;
	let conflict = $state.raw<ConflictState | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);

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

	function errorMessage(error: unknown) {
		if (error instanceof Error) return error.message;
		if (error && typeof error === 'object' && 'message' in error) {
			return String(error.message);
		}
		return 'Noura could not save this task.';
	}

	function showMessage(message: string) {
		transientMessage = message;
		if (clearMessageTimer) clearTimeout(clearMessageTimer);
		clearMessageTimer = setTimeout(() => {
			transientMessage = null;
			clearMessageTimer = null;
		}, 2400);
	}

	function currentDraft(): TaskDraft {
		return {
			title: draftTitle,
			body: editor?.doc() ?? baseBody,
			properties: { ...baseProperties },
		};
	}

	function draftInput(draft: TaskDraft) {
		return {
			id: selectedId ?? '',
			baseRevision,
			baseTitle,
			baseBody,
			baseProperties,
			localTitle: draft.title,
			localBody: draft.body,
			localProperties: draft.properties,
		};
	}

	function adoptSelection(task: Task) {
		selectedId = task.id;
		baseRevision = task.revision;
		baseTitle = task.title;
		baseBody = task.body;
		baseProperties = { ...task.properties };
		draftTitle = task.title;
		editor?.setText(task.body);
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
		} finally {
			loading = false;
		}
	}

	function select(task: Task) {
		if (selectedId === task.id) return;
		void adoptIfFlushed(task);
	}

	async function adoptIfFlushed(task: Task) {
		const flushed = await coordinator?.flush();
		if (flushed === false) return;
		adoptSelection(task);
	}

	function applyStatus(status: string) {
		if (!selected) return;
		baseProperties = { ...baseProperties, status };
		coordinator?.noteEdit(currentDraft());
		void coordinator?.flush();
	}

	function applyPriority(priority: string) {
		if (!selected) return;
		baseProperties = { ...baseProperties, priority };
		coordinator?.noteEdit(currentDraft());
		void coordinator?.flush();
	}

	function applyDue(due: string) {
		if (!selected) return;
		baseProperties = due
			? { ...baseProperties, due }
			: omitKey(baseProperties, 'due');
		coordinator?.noteEdit(currentDraft());
		void coordinator?.flush();
	}

	function applyProject(project: string) {
		if (!selected) return;
		baseProperties = project
			? { ...baseProperties, project }
			: omitKey(baseProperties, 'project');
		coordinator?.noteEdit(currentDraft());
		void coordinator?.flush();
	}

	function omitKey(
		properties: Record<string, unknown>,
		key: string,
	): Record<string, unknown> {
		const next = { ...properties };
		delete next[key];
		return next;
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
		const result = await getNouraClient().tasks.create({ title: 'New task' });
		await load();
		const created = result.value as Task;
		view = { mode: 'all' };
		select(created);
	}

	function mergeCanonical(task: Task) {
		tasks = tasks.map((entry) => (entry.id === task.id ? task : entry));
		baseRevision = task.revision;
		baseTitle = task.title;
		baseBody = task.body;
		baseProperties = { ...task.properties };
	}

	async function persistDraft(draft: TaskDraft, generation: number) {
		if (!selectedId) return;
		const result = await getNouraClient().tasks.saveDraft(draftInput(draft));
		if (result.status === 'conflict') {
			openConflict(draft, result.current as Task);
			return 'paused' as const;
		}
		if (result.status === 'unchanged') {
			mergeCanonical(result.current as Task);
			return;
		}
		const merged = result.body !== draft.body;
		mergeCanonical(result.current as Task);
		if (coordinator?.currentGeneration === generation) {
			draftTitle = result.title;
			editor?.setText(result.body);
		}
		if (merged) showMessage('External changes merged');
	}

	function editorContainer(node: HTMLDivElement) {
		if (!browser || !selected) return;
		const localCoordinator = new AutosaveCoordinator<TaskDraft>({
			write: persistDraft,
			onStateChange: (state) => {
				autosaveError = state.error;
			},
		});
		coordinator = localCoordinator;
		const document = createLiveMarkdownDocument('task-body', baseBody);
		const handle = createLiveMarkdownEditor(node, {
			ytext: document.ytext,
			resolveImage: (src) => {
				if (/^https?:|^(data|asset):/.test(src)) return null;
				return getNouraClient()
					.files.readLocalAsset({ relativePath: src.replace(/^\.\//, '') })
					.then((asset) => asset.dataUrl)
					.catch(() => null);
			},
			onChange: () => {
				localCoordinator.noteEdit({
					title: draftTitle,
					body: handle.doc(),
					properties: { ...baseProperties },
				});
			},
		});
		editor = handle;
		const unregister = registerPendingDraft(
			selected.id,
			() => localCoordinator.flush(),
			() =>
				localCoordinator.pendingEdits > 0 ||
				localCoordinator.isWriting ||
				localCoordinator.error !== null,
		);
		return () => {
			unregister();
			localCoordinator.destroy();
			handle.destroy();
			document.destroy();
			if (coordinator === localCoordinator) coordinator = null;
			if (editor === handle) editor = null;
		};
	}

	function openConflict(draft: TaskDraft, file: Task) {
		conflict = { draft, file };
		reviewOpen = true;
		coordinator?.pause();
	}

	async function handleExternalEvent(event: CoreEvent) {
		if (
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			!['object:updated', 'object:moved', 'object:deleted'].includes(event.type)
		)
			return;
		await load();
		const payload = event.payload as { id?: string };
		if (
			payload.id &&
			selectedId === payload.id &&
			event.type === 'object:deleted'
		) {
			showMessage('This task was deleted in the workspace');
		}
	}

	function requestResolution(resolution: Resolution) {
		pendingResolution = resolution;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !pendingResolution || !selectedId) return;
		resolving = true;
		try {
			const localDraft = conflict.draft;
			await getNouraClient().tasks.resolveManagedConflict({
				id: selectedId,
				currentRevision: conflict.file.revision,
				localTitle: localDraft.title,
				localBody: localDraft.body,
				localProperties: localDraft.properties,
				resolution: pendingResolution,
			});
			await load();
			coordinator?.acceptDurable();
			coordinator?.resume();
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			showMessage(
				pendingResolution === 'use-external'
					? 'File version restored'
					: 'File replaced',
			);
			pendingResolution = null;
		} catch (error) {
			autosaveError = error;
		} finally {
			resolving = false;
		}
	}

	function handleShortcut(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
			event.preventDefault();
			void coordinator?.flush();
		}
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
			if (clearMessageTimer) clearTimeout(clearMessageTimer);
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

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
			<div class="flex min-h-0 flex-1 flex-col">
				<header class="flex min-h-16 items-center px-6">
					<input
						bind:value={draftTitle}
						oninput={() => coordinator?.noteEdit(currentDraft())}
						onblur={() => void coordinator?.flush()}
						aria-label="Task title"
						placeholder="Task title"
						class="min-w-0 flex-1 bg-transparent text-lg font-semibold outline-none placeholder:text-muted-foreground"
					/>
				</header>
				<Separator />
				<div class="flex flex-wrap items-center gap-3 px-6 py-3 text-sm">
					<Select.Root
						type="single"
						value={String(selected.properties?.status ?? 'todo')}
						onValueChange={(value) => (value ? applyStatus(value) : null)}
					>
						<Select.Trigger size="sm" class="w-36" aria-label="Status"
							>{String(selected.properties?.status ?? 'todo')}</Select.Trigger
						>
						<Select.Content>
							{#each STATUSES as status (status)}<Select.Item
									value={status}
									label={status}>{status}</Select.Item
								>{/each}
						</Select.Content>
					</Select.Root>
					<Select.Root
						type="single"
						value={String(selected.properties?.priority ?? 'medium')}
						onValueChange={(value) => (value ? applyPriority(value) : null)}
					>
						<Select.Trigger size="sm" class="w-32" aria-label="Priority"
							>{String(
								selected.properties?.priority ?? 'medium',
							)}</Select.Trigger
						>
						<Select.Content>
							{#each PRIORITIES as priority (priority)}<Select.Item
									value={priority}
									label={priority}>{priority}</Select.Item
								>{/each}
						</Select.Content>
					</Select.Root>
					<label class="flex items-center gap-2 text-xs text-muted-foreground">
						<span>Due</span>
						<input
							type="date"
							value={typeof selected.properties?.due === 'string'
								? selected.properties.due
								: ''}
							onchange={(event) => applyDue(event.currentTarget.value)}
							class="rounded-md border border-input bg-transparent px-2 py-1"
						/>
					</label>
					<Select.Root
						type="single"
						value={typeof selected.properties?.project === 'string'
							? selected.properties.project
							: ''}
						onValueChange={applyProject}
					>
						<Select.Trigger size="sm" class="w-44" aria-label="Project"
							>{projectLabel(selected.properties) ||
								'No project'}</Select.Trigger
						>
						<Select.Content>
							<Select.Item value="" label="No project">No project</Select.Item>
							{#each projects as project (project.id)}<Select.Item
									value={project.id}
									label={project.title}>{project.title}</Select.Item
								>{/each}
						</Select.Content>
					</Select.Root>
					{#if transientMessage}<span
							class="text-xs text-muted-foreground"
							role="status">{transientMessage}</span
						>{/if}
				</div>
				{#if conflict}
					<div class="px-6 pb-3">
						<Alert.Root>
							<Warning />
							<Alert.Title>This task changed in another app</Alert.Title>
							<Alert.Description
								>Review both versions before choosing which one to keep.</Alert.Description
							>
							<Alert.Action
								><Button
									variant="outline"
									size="sm"
									onclick={() => (reviewOpen = true)}>Review conflict</Button
								></Alert.Action
							>
						</Alert.Root>
					</div>
				{/if}
				{#if autosaveError}
					<div class="px-6 pb-3">
						<Alert.Root variant="destructive">
							<Warning />
							<Alert.Title>Changes could not be saved</Alert.Title>
							<Alert.Description
								>{errorMessage(autosaveError)}</Alert.Description
							>
							<Alert.Action
								><Button
									variant="outline"
									size="sm"
									onclick={() => void coordinator?.flush()}>Retry</Button
								></Alert.Action
							>
						</Alert.Root>
					</div>
				{/if}
				<div class="min-h-0 flex-1 overflow-y-auto">
					<div class="mx-auto min-h-full max-w-3xl px-10 py-8">
						<div
							class="live-md min-h-full text-base"
							{@attach editorContainer}
						></div>
					</div>
				</div>
			</div>
		{/if}
	</main>
</div>

{#if conflict}
	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl">
			<Sheet.Header>
				<Sheet.Title>Review task conflict</Sheet.Title>
				<Sheet.Description
					>Noura preserved both versions of this task.</Sheet.Description
				>
			</Sheet.Header>
			<Sheet.Footer>
				<Button variant="outline" onclick={() => (reviewOpen = false)}
					>Cancel and continue reviewing</Button
				>
				<Button
					variant="outline"
					onclick={() => requestResolution('use-external')}
					>Use file version</Button
				>
				<Button onclick={() => requestResolution('replace-external')}
					>Replace file with my version</Button
				>
			</Sheet.Footer>
		</Sheet.Content>
	</Sheet.Root>
{/if}

<AlertDialog.Root bind:open={confirmOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title
				>{pendingResolution === 'use-external'
					? 'Use the file version?'
					: 'Replace the file version?'}</AlertDialog.Title
			>
			<AlertDialog.Description
				>The displaced version is saved to recovery history first.</AlertDialog.Description
			>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={resolving}>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				disabled={resolving}
				onclick={() => void resolveConflict()}>Continue</AlertDialog.Action
			>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
