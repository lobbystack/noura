<script lang="ts">
	import { onMount } from 'svelte';
	import { beforeNavigate, onNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import { ModeWatcher } from 'mode-watcher';
	import type {
		Note,
		Task,
		Project,
		WorkspaceState,
	} from '@noura/workspace/browser';
	import * as Select from '$lib/components/ui/select';
	import PropertyChoice from '$lib/components/property-choice.svelte';
	import { startBrowserWorkspace } from '$lib/browser-workspace';
	import { createBrowserPluginModel } from '$lib/browser-plugins';
	import type { PluginSettingsModel } from '$lib/plugin-settings-model';
	import PluginSettingsPanel from './settings/plugin-settings-panel.svelte';
	import {
		assertBackupActionAllowed,
		assertFreshIdentity,
		decodeBackup,
		downloadBackup,
		encodeBackup,
		IDENTITY_CONFLICT,
	} from '$lib/browser-backup';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Textarea } from '$lib/components/ui/textarea';
	import * as Field from '$lib/components/ui/field';
	let runtime: ReturnType<typeof startBrowserWorkspace> | undefined;
	let pluginModel: ReturnType<typeof createBrowserPluginModel> | undefined;
	let plugins = $state.raw<PluginSettingsModel | null>(null);
	const settingsRoute = $derived(page.url.pathname === '/settings');
	const routePlugin = $derived(
		['/', '/inbox'].includes(page.url.pathname)
			? 'notes'
			: page.url.pathname.slice(1),
	);
	const routeAllowed = $derived(!!plugins?.isEnabled(routePlugin));
	let ready = $state(false);
	let busy = $state(false);
	let error = $state('');
	let status = $state('');
	let workspaces = $state.raw<
		Array<{ path: string; name: string; workspaceId: string }>
	>([]);
	let workspace = $state.raw<WorkspaceState | null>(null);
	let notes = $state.raw<Note[]>([]);
	let tasks = $state.raw<Task[]>([]);
	let projects = $state.raw<Project[]>([]);
	let summaries = $state.raw<Array<{ project: Project; taskCount: number }>>(
		[],
	);
	let projectTasks = $state.raw<Task[]>([]);
	let projectId = $state('');
	let projectStatus = $state('planned');
	let selected = $state.raw<Note | Task | Project | null>(null);
	let taskStatus = $state('todo');
	let priority = $state('medium');
	let due = $state('');
	let statusFilter = $state('all');
	const taskRoute = $derived(page.url.pathname === '/tasks');
	const projectRoute = $derived(page.url.pathname === '/projects');
	const kind = $derived(projectRoute ? 'project' : taskRoute ? 'task' : 'note');
	const projectStatuses = [
		'planned',
		'active',
		'on-hold',
		'completed',
		'cancelled',
	];
	const statuses = ['todo', 'in-progress', 'done', 'cancelled'];
	const priorities = ['low', 'medium', 'high', 'urgent'];
	function service() {
		return projectRoute
			? runtime!.client.projects
			: taskRoute
				? runtime!.client.tasks
				: runtime!.client.notes;
	}
	let editing = $state(false);
	let title = $state('');
	let body = $state('');
	let name = $state('');
	let filter = $state('');
	let movePath = $state('');
	const dirty = $derived(
		editing &&
			(title !== (selected?.title ?? '') ||
				body !== (selected?.body ?? '') ||
				(projectRoute &&
					projectStatus !== (selected?.properties.status ?? 'planned')) ||
				(taskRoute &&
					(projectId !== (selected?.properties.project ?? '') ||
						taskStatus !== (selected?.properties.status ?? 'todo') ||
						priority !== (selected?.properties.priority ?? 'medium') ||
						due !== (selected?.properties.due ?? '')))),
	);
	const visible = $derived(
		(projectRoute ? projects : taskRoute ? tasks : notes).filter(
			(note) =>
				note.title.toLowerCase().includes(filter.toLowerCase()) &&
				(!taskRoute ||
					statusFilter === 'all' ||
					note.properties.status === statusFilter),
		),
	);
	const supportedRoute = $derived(
		['/', '/notes', '/inbox', '/tasks', '/projects'].includes(
			page.url.pathname,
		),
	);
	function draftKey(path: string) {
		return ['/', '/inbox', '/notes'].includes(path) ? '/notes' : path;
	}
	function captureDraft() {
		return {
			selected,
			editing,
			title,
			body,
			taskStatus,
			priority,
			due,
			projectId,
			projectStatus,
			movePath,
			projectTasks,
			dirty,
		};
	}
	let drafts = $state.raw<Record<string, ReturnType<typeof captureDraft>>>({});
	const hasDrafts = $derived(
		dirty ||
			Object.entries(drafts).some(
				([key, draft]) => key !== draftKey(page.url.pathname) && draft.dirty,
			),
	);
	const workspaceName = $derived(
		workspaces.find((item) => item.workspaceId === workspace?.workspaceId)
			?.name ?? 'Browser workspace',
	);
	function canLeave() {
		return !busy && (!dirty || window.confirm('Discard unsaved changes?'));
	}
	beforeNavigate((navigation) => {
		const internal =
			navigation.to?.route.id?.startsWith('/(workspace)') &&
			!navigation.willUnload;
		if (
			busy ||
			(!internal &&
				hasDrafts &&
				!window.confirm('Discard all unsaved browser drafts and leave?'))
		)
			navigation.cancel();
	});
	onNavigate((navigation) => {
		if (navigation.to && navigation.to.url.pathname !== page.url.pathname) {
			if (supportedRoute)
				drafts = { ...drafts, [draftKey(page.url.pathname)]: captureDraft() };
			const target = draftKey(navigation.to.url.pathname);
			return () => {
				reset();
				const draft = drafts[target];
				if (draft)
					({
						selected,
						editing,
						title,
						body,
						taskStatus,
						priority,
						due,
						projectId,
						projectStatus,
						movePath,
						projectTasks,
					} = draft);
				if (supportedRoute && ready) void run(refresh);
			};
		}
	});
	async function run(action: () => Promise<void>) {
		if (busy) return;
		busy = true;
		error = '';
		status = '';
		try {
			await action();
		} catch (cause) {
			error =
				typeof cause === 'object' &&
				cause !== null &&
				'code' in cause &&
				cause.code === 'revision_conflict'
					? 'Stale revision: the saved file changed. Nothing was overwritten. Your draft is kept; copy it before using Discard draft / reload saved file, then review and reapply your changes.'
					: typeof cause === 'object' && cause !== null && 'message' in cause
						? String(cause.message)
						: 'The operation failed. Your draft has been kept.';
		} finally {
			busy = false;
		}
	}
	function reset(note: Note | Task | Project | null = null) {
		projectTasks = [];
		projectStatus = String(note?.properties.status ?? 'planned');
		projectId =
			typeof note?.properties.project === 'string'
				? note.properties.project
				: '';
		selected = note;
		movePath = note?.relativePath ?? '';
		title = note?.title ?? '';
		body = note?.body ?? '';
		taskStatus = String(note?.properties.status ?? 'todo');
		priority = String(note?.properties.priority ?? 'medium');
		due = typeof note?.properties.due === 'string' ? note.properties.due : '';
		editing = note !== null;
	}
	async function refresh() {
		workspace = await runtime!.client.workspaces.current();
		workspaces = await runtime!.client.workspaces.listRecent();
		await pluginModel!.sync();
		notes = plugins?.isEnabled('notes')
			? await runtime!.client.notes.list()
			: [];
		tasks = plugins?.isEnabled('tasks')
			? await runtime!.client.tasks.list()
			: [];
		projects = plugins?.isEnabled('projects')
			? await runtime!.client.projects.list()
			: [];
		summaries =
			plugins?.isEnabled('projects') && plugins?.isEnabled('tasks')
				? await runtime!.client.projects.listSummaries()
				: [];
		if (
			projectRoute &&
			selected &&
			plugins?.isEnabled('projects') &&
			plugins?.isEnabled('tasks')
		)
			projectTasks = await runtime!.client.projects.listTasks({
				projectId: selected.id,
			});
	}
	function open(path?: string) {
		if (
			busy ||
			(hasDrafts &&
				!window.confirm('Discard all unsaved drafts and switch workspace?'))
		)
			return;
		void run(async () => {
			await pluginModel!.scope(async () => {
				workspace = path
					? await runtime!.client.workspaces.open({ path })
					: await runtime!.client.workspaces.create({
							path: 'browser://',
							name,
						});
			});
			drafts = {};
			reset();
			name = '';
			await refresh();
		});
	}
	function select(id: string) {
		if (!routeAllowed) return;
		if (!canLeave()) return;
		void run(async () => {
			reset(await service().get(id));
			if (projectRoute && plugins?.isEnabled('tasks'))
				projectTasks = await runtime!.client.projects.listTasks({
					projectId: id,
				});
		});
	}
	function save() {
		if (!routeAllowed) return;
		void run(async () => {
			const properties = projectRoute
				? { status: projectStatus }
				: taskRoute
					? {
							status: taskStatus,
							priority,
							...(due ? { due } : {}),
							...(projectId ? { project: projectId } : {}),
						}
					: undefined;
			const result = selected
				? await service().update(selected.id, {
						title,
						body,
						properties,
						removeProperties: taskRoute
							? [...(!due ? ['due'] : []), ...(!projectId ? ['project'] : [])]
							: [],
						expectedRevision: selected.revision,
					})
				: await service().create({ title, body, properties });
			reset(result.value);
			status = 'Saved to browser storage.';
			await refresh();
		});
	}
	function moveNote() {
		if (!routeAllowed) return;
		void run(async () => {
			if (dirty)
				throw new Error(`Save or discard your draft before moving a ${kind}.`);
			if (!selected) return;
			const result = await service().move({
				id: selected.id,
				relativePath: movePath,
				expectedRevision: selected.revision,
			});
			reset(result.value);
			status = projectRoute
				? 'Project moved. Content and stable ID preserved. Task relationships are unchanged.'
				: `${taskRoute ? 'Task' : 'Note'} moved. Content preserved.`;
			await refresh();
		});
	}
	function trashNote() {
		if (!routeAllowed) return;
		void run(async () => {
			if (dirty)
				throw new Error(
					`Save or discard your draft before moving a ${kind} to trash.`,
				);
			if (!selected) return;
			const note = selected;
			if (
				!window.confirm(
					`Move “${note.title}” (${note.relativePath}) to trash? ${projectRoute ? 'Only the project file is trashed. Tasks and other files are not deleted or moved. Task project IDs remain and will refer to an unavailable project. ' : ''}It will leave the live ${kind}s list. Trash is retained in JSON backups, but restoring a trashed ${kind} in the UI is not yet available.`,
				)
			) {
				status = 'Move to trash cancelled. File unchanged.';
				return;
			}
			await service().delete({
				id: note.id,
				expectedRevision: note.revision,
			});
			notes = notes.filter((item) => item.id !== note.id);
			tasks = tasks.filter((item) => item.id !== note.id);
			projects = projects.filter((item) => item.id !== note.id);
			reset();
			status = `${projectRoute ? 'Project' : taskRoute ? 'Task' : 'Note'} moved to trash.${projectRoute ? ' No cascade: tasks and other files are unchanged.' : ''} Trash is retained in JSON backups; UI restore is not yet available.`;
		});
	}
	function toggleComplete() {
		if (!routeAllowed) return;
		void run(async () => {
			if (!selected || selected.type !== 'task' || dirty)
				throw new Error('Save or discard your draft first.');
			const input = { id: selected.id, expectedRevision: selected.revision };
			const result =
				selected.properties.status === 'done'
					? await runtime!.client.tasks.reopen(input)
					: await runtime!.client.tasks.complete(input);
			reset(result.value);
			status = 'Task status saved to browser storage.';
			await refresh();
		});
	}
	function backup() {
		void run(async () => {
			assertBackupActionAllowed(hasDrafts);
			const snapshot = await runtime!.exportWorkspace();
			downloadBackup(encodeBackup(snapshot), workspaceName);
			status =
				'Backup download requested. Check your downloads before relying on it. The backup is not encrypted; store it securely.';
		});
	}
	function importBackup(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		void run(async () => {
			assertBackupActionAllowed(hasDrafts);
			const snapshot = await decodeBackup(file);
			assertFreshIdentity(
				snapshot.workspaceId,
				await runtime!.client.workspaces.listRecent(),
			);
			if (
				!window.confirm(
					`Import ${snapshot.entries.length} files as workspace ${snapshot.workspaceId}? This opens the restored workspace without replacing existing workspaces. The original stable identity is preserved.`,
				)
			) {
				status = 'Import cancelled. No workspace changed.';
				return;
			}
			try {
				await pluginModel!.scope(async () => {
					workspace = await runtime!.importWorkspace(snapshot);
				});
			} catch (cause) {
				if (
					typeof cause === 'object' &&
					cause !== null &&
					'code' in cause &&
					cause.code === 'workspace_not_empty'
				)
					throw new Error(IDENTITY_CONFLICT);
				throw cause;
			}
			drafts = {};
			reset();
			await refresh();
			status =
				'Backup imported and opened. Existing workspaces were not overwritten.';
		});
	}
	onMount(() => {
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		const timer = setInterval(() => {
			if (ready && !busy) void pluginModel?.sync();
		}, 2000);
		void run(async () => {
			runtime = startBrowserWorkspace();
			await runtime.ready;
			if (disposed) return;
			pluginModel = createBrowserPluginModel(runtime.client, runtime.plugins);
			unsubscribe = pluginModel.subscribe((model) => {
				plugins = model;
			});
			await pluginModel.init();
			await refresh();
			ready = true;
		});
		return () => {
			disposed = true;
			clearInterval(timer);
			unsubscribe?.();
			void (async () => {
				try {
					await pluginModel?.dispose();
				} finally {
					await runtime?.dispose();
				}
			})().catch(() => {});
		};
	});
</script>

<svelte:head><title>{workspaceName} · Noura</title></svelte:head>
<svelte:window
	onbeforeunload={(event) => {
		if (hasDrafts || busy) {
			event.preventDefault();
			event.returnValue = '';
		}
	}}
/>
<ModeWatcher />
<main class="mx-auto flex min-h-svh max-w-6xl flex-col gap-6 p-6">
	<header class="flex flex-wrap items-center justify-between gap-4">
		<div>
			<h1 class="text-base font-semibold">Noura · Browser workspace</h1>
			<p class="text-sm text-muted-foreground">{workspaceName}</p>
		</div>
		<a href="/account">Account</a>
	</header>
	<p class="text-sm text-muted-foreground">
		Files are stored in this browser’s private filesystem on this device.
		Clearing site data can erase them. This is not a backup or sync service.
		Drafts are kept while navigating plugins in this tab, including while
		disabled. Save before reloading, closing or switching workspaces.
	</p>
	{#if error}<p role="alert" class="text-destructive">
			{error}
			{dirty
				? 'Your unsaved draft is still here. Copy it before reloading or discarding.'
				: ''}
		</p>{/if}
	<p role="status">{busy ? 'Working…' : status}</p>
	{#if !ready}
		<p>
			{error
				? 'Reload the page to retry initialization.'
				: 'Starting browser storage…'}
		</p>
	{:else}
		<section aria-label="Browser workspaces" class="flex flex-col gap-4">
			<section aria-label="Backup and restore" class="flex flex-col gap-3">
				<h2>Backup and restore</h2>
				<p id="backup-help" class="text-sm text-muted-foreground">
					Lossless, versioned JSON backup (not ZIP), including binary files. Not
					encrypted; keep it private. Limits: 96 MiB backup, 10,000 files, 32
					MiB per file, 64 MiB decoded total. Save or discard drafts first.
					Import never overwrites an existing workspace identity. Trash is
					retained in backups, but restoring individual trashed files in the UI
					is not yet available.
				</p>
				<Button
					variant="outline"
					disabled={busy || hasDrafts || workspace?.phase !== 'ready'}
					onclick={backup}>Download JSON backup</Button
				>
				<Field.Field>
					<Field.Label for="backup-file">Import Noura JSON backup</Field.Label>
					<Input
						id="backup-file"
						type="file"
						accept=".noura-backup.json,application/json"
						aria-describedby="backup-help"
						disabled={busy || hasDrafts}
						onchange={importBackup}
					/>
				</Field.Field>
			</section>
			<form
				onsubmit={(event) => {
					event.preventDefault();
					open();
				}}
			>
				<Field.Group
					><Field.Field
						><Field.Label for="workspace-name">New workspace name</Field.Label
						><Input
							id="workspace-name"
							aria-label="New workspace name"
							bind:value={name}
							disabled={busy}
							required
						/></Field.Field
					>
					<Button type="submit" disabled={busy || !name.trim()}
						>Create browser workspace</Button
					></Field.Group
				>
			</form>
			<div class="flex flex-wrap gap-2">
				{#each workspaces as item (item.workspaceId)}<Button
						variant="outline"
						disabled={busy}
						onclick={() => open(item.path)}>Open {item.name}</Button
					>{/each}
				<Button variant="outline" disabled={busy} onclick={() => run(refresh)}
					>Refresh workspaces</Button
				>
				<Button
					variant="outline"
					disabled={busy || workspace?.phase !== 'ready'}
					onclick={() => {
						if (
							hasDrafts &&
							!window.confirm('Discard all unsaved drafts and close workspace?')
						)
							return;
						void run(async () => {
							await pluginModel!.scope(async () => {
								await runtime!.client.workspaces.close();
							});
							drafts = {};
							reset();
							await refresh();
						});
					}}>Close workspace</Button
				>
			</div>
		</section>
		<nav aria-label="Browser features" class="flex flex-wrap gap-2">
			{#each plugins?.catalog ?? [] as plugin (plugin.id)}
				{#if plugins?.isEnabled(plugin.id)}<a href={`/${plugin.id}`}
						>{plugin.name}</a
					>{/if}
			{/each}
			<a href="/settings">Plugin settings</a>
		</nav>
		{#if plugins?.lastError}<p role="alert">{plugins.lastError}</p>{/if}
		{#if hasDrafts}<p role="status">
				Unsaved drafts are retained in this tab. Re-enable the plugin and return
				to its page to continue editing.
			</p>{/if}
		{#if settingsRoute && plugins}
			<section aria-label="Plugin settings">
				<h2>Plugins</h2>
				<Button variant="outline" disabled={busy} onclick={() => run(refresh)}
					>Refresh plugins</Button
				>
				<PluginSettingsPanel
					{plugins}
					workspaceReady={workspace?.phase === 'ready'}
				/>
			</section>
		{:else if !supportedRoute || (workspace?.phase === 'ready' && !routeAllowed)}<p
				role="status"
			>
				{!supportedRoute || !plugins?.isSupported(routePlugin)
					? 'This feature is not supported in browser workspaces.'
					: plugins?.lastError
						? 'This plugin is blocked until runtime reconciliation succeeds.'
						: 'This plugin is disabled or not active in this workspace.'}
				Your drafts have not been discarded.
				<a href="/settings">Open plugin settings</a>.
			</p>
		{:else if workspace?.phase === 'ready'}
			<div class="grid gap-6 md:grid-cols-[16rem_1fr]">
				<aside
					class="flex flex-col gap-3"
					aria-label={projectRoute ? 'Projects' : taskRoute ? 'Tasks' : 'Notes'}
				>
					<Button
						disabled={busy}
						onclick={() => {
							if (canLeave()) {
								reset();
								editing = true;
							}
						}}>New {kind}</Button
					>
					<Field.Field
						><Field.Label for="note-filter">Filter {kind} titles</Field.Label
						><Input
							id="note-filter"
							aria-label={`Filter ${kind} titles`}
							bind:value={filter}
						/></Field.Field
					>
					{#if taskRoute}
						<Select.Root type="single" bind:value={statusFilter}>
							<Select.Trigger aria-label="Filter task status"
								>{statusFilter === 'all'
									? 'All statuses'
									: statusFilter}</Select.Trigger
							>
							<Select.Content
								><Select.Group>
									{#each ['all', ...statuses] as value (value)}<Select.Item
											{value}>{value}</Select.Item
										>{/each}
								</Select.Group></Select.Content
							>
						</Select.Root>
					{/if}
					{#each visible as note (note.id)}<Button
							variant="outline"
							disabled={busy}
							onclick={() => select(note.id)}
							>{note.title}{projectRoute
								? ` · ${note.properties.status} · ${summaries.find((item) => item.project.id === note.id)?.taskCount ?? '—'} tasks`
								: taskRoute
									? ` · ${note.properties.status}`
									: ''}</Button
						>{:else}<p>No {kind}s found.</p>{/each}
					<Button
						variant="outline"
						disabled={busy}
						onclick={() =>
							run(async () => {
								workspace = await runtime!.client.workspaces.rebuildIndex();
								await refresh();
								status = 'Rebuilt from browser files. Draft unchanged.';
							})}>Rebuild / refresh {kind}s</Button
					>
				</aside>
				<section
					aria-label={projectRoute
						? 'Project editor'
						: taskRoute
							? 'Task editor'
							: 'Note editor'}
					class="min-w-0"
				>
					{#if editing}
						<form
							onsubmit={(event) => {
								event.preventDefault();
								save();
							}}
						>
							<Field.Group>
								<Field.Field
									><Field.Label for="note-title">Title</Field.Label><Input
										id="note-title"
										aria-label="Title"
										bind:value={title}
										required
										disabled={busy}
									/></Field.Field
								>
								<Field.Field
									><Field.Label for="note-body">Markdown</Field.Label><Textarea
										id="note-body"
										aria-label="Markdown"
										bind:value={body}
										rows={16}
										disabled={busy}
									/></Field.Field
								>
								{#if projectRoute}
									<Field.Field>
										<Field.Label for="project-status">Status</Field.Label>
										<PropertyChoice
											id="project-status"
											label="Status"
											value={projectStatus}
											options={projectStatuses}
											disabled={busy}
											onchange={(value) => (projectStatus = value)}
										/>
									</Field.Field>
								{/if}
								{#if taskRoute}
									<Field.Field>
										<Field.Label for="task-project">Project</Field.Label>
										<Select.Root
											type="single"
											value={projectId || 'none'}
											onValueChange={(value) =>
												(projectId = value === 'none' ? '' : value)}
											disabled={busy || !plugins?.isEnabled('projects')}
										>
											<Select.Trigger id="task-project" aria-label="Project"
												>{projectId
													? (projects.find((item) => item.id === projectId)
															?.title ?? `Unavailable project (${projectId})`)
													: 'No project'}</Select.Trigger
											>
											<Select.Content
												><Select.Group>
													<Select.Item value="none">No project</Select.Item>
													{#each projects as project (project.id)}<Select.Item
															value={project.id}
															>{project.title} · {project.id}</Select.Item
														>{/each}
												</Select.Group></Select.Content
											>
										</Select.Root>
										<p class="text-sm text-muted-foreground">
											Membership uses the stable project ID, not its path. An
											unavailable project is preserved unless you change or
											clear it.
										</p>
									</Field.Field>
									<Field.Field
										><Field.Label for="task-status">Status</Field.Label>
										<PropertyChoice
											id="task-status"
											label="Status"
											value={taskStatus}
											options={statuses}
											disabled={busy}
											onchange={(value) => (taskStatus = value)}
										/></Field.Field
									>
									<Field.Field
										><Field.Label for="task-priority">Priority</Field.Label>
										<PropertyChoice
											id="task-priority"
											label="Priority"
											value={priority}
											options={priorities}
											disabled={busy}
											onchange={(value) => (priority = value)}
										/></Field.Field
									>
									<Field.Field
										><Field.Label for="task-due">Due</Field.Label>
										<Input
											id="task-due"
											bind:value={due}
											disabled={busy}
											aria-describedby="task-due-help"
										/>
										<p id="task-due-help">
											YYYY-MM-DD or RFC 3339 timestamp with offset. Leave blank
											to clear. Dates are validated when saved.
										</p>
									</Field.Field>
									{#if selected}<Button
											type="button"
											variant="outline"
											disabled={busy || dirty}
											onclick={toggleComplete}
											>{selected.properties.status === 'done'
												? 'Reopen task'
												: 'Complete task'}</Button
										>{/if}
								{/if}
								<p>
									{dirty ? 'Unsaved changes' : 'No unsaved changes'}{selected
										? ` · ${selected.relativePath}`
										: ''}
								</p>
								<p class="text-sm text-muted-foreground">
									Saves, moves and trash use the loaded revision. If the file
									changed elsewhere, your operation is rejected; copy your
									draft, then discard / reload the saved file to review it. No
									automatic overwrite or merge.
								</p>
								<Button
									type="submit"
									disabled={busy || !title.trim() || (!dirty && !!selected)}
									>Save {kind}</Button
								>
								<Button
									type="button"
									variant="outline"
									disabled={busy}
									onclick={() => {
										if (canLeave()) {
											if (selected) {
												const id = selected.id;
												void run(async () => {
													reset(await service().get(id));
													if (projectRoute && plugins?.isEnabled('tasks'))
														projectTasks =
															await runtime!.client.projects.listTasks({
																projectId: id,
															});
												});
											} else reset();
										}
									}}>Discard draft / reload saved {kind}</Button
								>
							</Field.Group>
						</form>
						{#if selected}
							<form
								class="mt-4"
								onsubmit={(event) => {
									event.preventDefault();
									moveNote();
								}}
							>
								<Field.Group>
									<p id="note-actions-help">
										Save or discard unsaved changes before moving or trashing a
										{kind}.
									</p>
									<Field.Field>
										<Field.Label for="move-path">Destination path</Field.Label>
										<Input
											id="move-path"
											bind:value={movePath}
											required
											disabled={busy || dirty}
											aria-describedby="move-path-help note-actions-help"
										/>
										<p
											id="move-path-help"
											class="text-sm text-muted-foreground"
										>
											Workspace-relative Markdown path, for example
											{projectRoute
												? 'Projects/Archive/project.md'
												: taskRoute
													? 'Tasks/Archive/task.md'
													: 'Notes/Archive/note.md'}. Existing files will not be
											overwritten.
										</p>
									</Field.Field>
									<Button
										type="submit"
										variant="outline"
										disabled={busy ||
											dirty ||
											!movePath.trim() ||
											movePath === selected.relativePath}>Move {kind}</Button
									>
									<p id="trash-help" class="text-sm text-muted-foreground">
										{projectRoute
											? 'No cascade: only the project file is trashed. Tasks and other files stay in place; task project IDs are retained and may refer to an unavailable project.'
											: ''}
										Trash is retained in JSON backups. Restoring a trashed file in
										the UI is not yet available.
									</p>
									<Button
										type="button"
										variant="destructive"
										disabled={busy || dirty}
										aria-describedby="trash-help note-actions-help"
										onclick={trashNote}>Move to trash</Button
									>
								</Field.Group>
							</form>
						{/if}
						{#if projectRoute && selected && plugins?.isEnabled('tasks')}
							<section
								aria-label="Project tasks"
								class="mt-4 flex flex-col gap-2"
							>
								<h2>Project tasks</h2>
								<p>Stable project ID: {selected.id}</p>
								{#each projectTasks as task (task.id)}<p>
										{task.title} · {String(task.properties.status)}
									</p>{:else}<p>No linked tasks.</p>{/each}
								<a href="/tasks">Edit task membership in Tasks</a>
							</section>
						{/if}
					{:else}<p>
							Select a {kind} or create one. Plain Markdown editing is supported;
							attachments, autosave, and automatic conflict merging are not.
						</p>{/if}
				</section>
			</div>
			{#each workspace.diagnostics as diagnostic, index (`${index}:${diagnostic.code}`)}<p
					role="alert"
				>
					{diagnostic.message}
				</p>{/each}
		{:else}<p>
				Create or open a browser workspace to write notes, tasks and projects.
			</p>{/if}
	{/if}
</main>
