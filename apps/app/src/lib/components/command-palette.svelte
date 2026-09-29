<script lang="ts">
	import { goto } from '$app/navigation';
	import { onDestroy } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { DebouncedSearch } from '$lib/debounced-search';
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import { routePlugin } from '$lib/plugin-routes';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import {
		collectFilePaths,
		displayName,
		findTreeNode,
	} from '$lib/workspace-tree';
	import { quickOpen } from '$lib/quick-open';
	import { openTreeNode } from '$lib/file-actions';
	import {
		navigationHref,
		searchResultIsVisible,
		searchResultTarget,
	} from '$lib/navigation-targets';
	import * as Command from '$lib/components/ui/command';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Checks from 'phosphor-svelte/lib/Checks';
	import File from 'phosphor-svelte/lib/File';
	import FilePdf from 'phosphor-svelte/lib/FilePdf';
	import FileText from 'phosphor-svelte/lib/FileText';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import type { SearchResult } from '@noura/workspace';

	const settings = getSettingsDialog();

	let query = $state('');
	let results = $state<SearchResult[]>([]);
	let searching = $state(false);
	let searchError = $state<string | null>(null);
	const search = new DebouncedSearch<SearchResult>({
		delayMs: 150,
		search: (searchText) =>
			getNouraClient().search.query({ query: searchText, limit: 8 }),
		onChange: (state) => {
			results = state.results;
			searching = state.searching;
			searchError = state.error;
		},
	});
	onDestroy(() => search.dispose());

	const trimmed = $derived(query.trim());
	const mode = $derived(commandPalette.mode);
	const filePaths = $derived(collectFilePaths(workspaceTree.tree));
	// Quick open runs on the paths the file tree already holds: no round trip.
	const fileMatches = $derived(
		trimmed.length > 0 && mode !== 'search'
			? quickOpen(filePaths, trimmed, mode === 'files' ? 12 : 6)
			: [],
	);
	const contentResults = $derived.by(() => {
		if (mode === 'files') return [];
		const shown = new Set(fileMatches.map((match) => match.path));
		const enabled = new Set(plugins.activeIds);
		return results.filter(
			(result) =>
				!shown.has(result.relativePath) &&
				searchResultIsVisible(result, enabled),
		);
	});

	const NAV_ENTRIES: ReadonlyArray<readonly [string, string, string | null]> = (
		[
			['Home', '/inbox'],
			['Files', '/files'],
			['Tasks', '/tasks'],
			['Calendar', '/calendar'],
			['Projects', '/projects'],
			['AI', '/ai'],
			['Settings', '/settings'],
		] as const
	).map(([label, path]) => [label, path, routePlugin(path)] as const);

	const placeholder = $derived(
		mode === 'files'
			? 'Open a file…'
			: mode === 'search'
				? 'Search file contents…'
				: 'Search or type to create…',
	);

	function setQuery(value: string) {
		query = value;
		if (mode !== 'files') search.update(value);
	}

	function close() {
		commandPalette.close();
		query = '';
		search.reset();
	}

	function fileIcon(path: string) {
		if (/\.pdf$/i.test(path)) return FilePdf;
		const node = findTreeNode(workspaceTree.tree, path);
		if (node?.objectType === 'note') return NotePencil;
		if (node?.objectType === 'task') return Checks;
		if (node?.objectType === 'project') return FolderOpen;
		return /\.md$/i.test(path) ? FileText : File;
	}

	function resultIcon(result: SearchResult) {
		if (result.objectType === 'note') return NotePencil;
		if (result.objectType === 'task') return Checks;
		if (result.objectType === 'project') return FolderOpen;
		return FileText;
	}

	async function openFile(path: string) {
		const node = findTreeNode(workspaceTree.tree, path);
		close();
		if (node) await openTreeNode(node);
	}

	async function create(command: 'notes.create' | 'tasks.create') {
		const title = trimmed;
		close();
		try {
			const created = (await getNouraClient().commands.execute(command, {
				title,
			})) as { id?: string } | null | undefined;
			if (!created?.id) return;
			const id = encodeURIComponent(created.id);
			await goto(
				command === 'notes.create'
					? `/files?selected=${id}`
					: `/tasks?view=all&selected=${id}`,
			);
			toast.success(`Created “${title}”`);
		} catch {
			toast.error(
				command === 'notes.create'
					? 'Couldn’t create the note.'
					: 'Couldn’t create the task.',
			);
		}
	}

	async function openResult(result: SearchResult) {
		const target = searchResultTarget(result);
		if (!target || !plugins.isEnabled(target.pluginId)) return;
		close();
		await goto(navigationHref(target));
	}

	async function openRoute(route: string) {
		close();
		if (route === '/settings') settings.show();
		else await goto(route);
	}

	function handleKeydown(event: KeyboardEvent) {
		if (
			(event.metaKey || event.ctrlKey) &&
			!event.shiftKey &&
			!event.altKey &&
			event.key.toLowerCase() === 'k'
		) {
			event.preventDefault();
			if (commandPalette.open) close();
			else commandPalette.show();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<Command.Dialog
	contentProps={{
		onCloseAutoFocus: (event) => {
			if (settings.open) event.preventDefault();
		},
	}}
	bind:open={
		() => commandPalette.open,
		(open) => {
			if (open) commandPalette.show(mode);
			else close();
		}
	}
	shouldFilter={false}
	title="Search and commands"
	description="Open files, search the workspace, or create something"
	class="max-w-xl"
>
	<Command.Input bind:value={() => query, setQuery} {placeholder} />
	<Command.List
		{@attach () => {
			// Quick open needs the file list even before the tree was shown.
			void workspaceTree.start(workspace.state?.workspaceId);
		}}
	>
		{#if trimmed.length === 0}
			{#if mode === 'all'}
				<Command.Group heading="Go to">
					{#each NAV_ENTRIES as [label, route, pluginId] (route)}
						{#if !pluginId || plugins.isEnabled(pluginId)}
							<Command.Item
								value="go:{route}"
								onSelect={() => void openRoute(route)}
							>
								<span>{label}</span>
							</Command.Item>
						{/if}
					{/each}
				</Command.Group>
			{/if}
		{:else}
			{#if fileMatches.length > 0}
				<Command.Group heading="Files">
					{#each fileMatches as match (match.path)}
						{@const Icon = fileIcon(match.path)}
						{@const name = match.path.slice(match.path.lastIndexOf('/') + 1)}
						<Command.Item
							value="file:{match.path}"
							onSelect={() => void openFile(match.path)}
						>
							<Icon class="shrink-0 text-muted-foreground" />
							<div class="min-w-0 flex-1">
								<p class="truncate font-medium">
									{displayName({ kind: 'file', name })}
								</p>
								{#if match.path.includes('/')}
									<p class="truncate text-xs text-muted-foreground">
										{match.path.slice(0, match.path.lastIndexOf('/'))}
									</p>
								{/if}
							</div>
						</Command.Item>
					{/each}
				</Command.Group>
			{/if}
			{#if contentResults.length > 0}
				<Command.Group heading="In files">
					{#each contentResults as result (result.relativePath)}
						{@const Icon = resultIcon(result)}
						<Command.Item
							value="result:{result.relativePath}"
							onSelect={() => void openResult(result)}
						>
							<Icon class="shrink-0 text-muted-foreground" />
							<div class="min-w-0 flex-1">
								<div class="flex items-center gap-2">
									<span class="truncate font-medium">{result.title}</span>
									{#if result.objectType}
										<Badge variant="outline" class="text-xs capitalize"
											>{result.objectType}</Badge
										>
									{/if}
								</div>
								<p class="truncate text-xs text-muted-foreground">
									{result.relativePath}
								</p>
							</div>
						</Command.Item>
					{/each}
				</Command.Group>
			{/if}
			{#if mode !== 'files' && searching && results.length === 0}
				<Command.Loading>Searching…</Command.Loading>
			{/if}
			{#if mode !== 'files' && searchError}
				<p class="px-3 py-4 text-xs text-destructive" role="alert">
					Search isn’t available right now.
				</p>
			{:else if fileMatches.length === 0 && contentResults.length === 0 && !searching}
				<p class="px-3 py-4 text-xs text-muted-foreground">
					No matches for “{trimmed}”.
				</p>
			{/if}
			{#if mode !== 'search'}
				<Command.Group heading="Create">
					<Command.Item
						value="action:new-note"
						onSelect={() => void create('notes.create')}
					>
						<NotePencil class="text-muted-foreground" />
						<span class="truncate">New note “{trimmed}”</span>
					</Command.Item>
					{#if mode === 'all' && plugins.isEnabled('tasks')}
						<Command.Item
							value="action:new-task"
							onSelect={() => void create('tasks.create')}
						>
							<Checks class="text-muted-foreground" />
							<span class="truncate">New task “{trimmed}”</span>
						</Command.Item>
					{/if}
				</Command.Group>
			{/if}
			{#if mode === 'all' && 'settings'.startsWith(trimmed.toLowerCase())}
				<Command.Group heading="Go to">
					<Command.Item
						value="action:settings"
						onSelect={() => void openRoute('/settings')}>Settings</Command.Item
					>
				</Command.Group>
			{/if}
		{/if}
	</Command.List>
</Command.Dialog>
