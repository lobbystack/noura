<script lang="ts">
	import { goto } from '$app/navigation';
	import { onDestroy } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { DebouncedSearch } from '$lib/debounced-search';
	import { getNouraClient } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import {
		navigationHref,
		searchResultIsVisible,
		searchResultTarget,
	} from '$lib/navigation-targets';
	import * as Command from '$lib/components/ui/command';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Checks from 'phosphor-svelte/lib/Checks';
	import FileText from 'phosphor-svelte/lib/FileText';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import type { SearchResult } from '@noura/workspace';

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
	const visibleResults = $derived(
		results.filter((result) =>
			searchResultIsVisible(result, new Set(plugins.activeIds)),
		),
	);

	const NAV_ENTRIES: ReadonlyArray<readonly [string, string, string | null]> = [
		['Inbox', '/inbox', null],
		['Notes', '/notes', 'notes'],
		['Tasks', '/tasks', 'tasks'],
		['Calendar', '/calendar', 'calendar'],
		['Projects', '/projects', 'projects'],
		['Settings', '/settings', null],
	];

	function resultIcon(result: SearchResult) {
		if (result.objectType === 'note') return NotePencil;
		if (result.objectType === 'task') return Checks;
		if (result.objectType === 'project') return FolderOpen;
		return FileText;
	}

	// Fresh open: clear stale state so the palette always starts home.
	$effect(() => {
		if (commandPalette.open) {
			query = '';
			search.reset();
		} else {
			search.reset();
		}
	});

	// Debounced live FTS while typing.
	$effect(() => {
		const open = commandPalette.open;
		const value = query;
		if (open) search.update(value);
	});

	onDestroy(() => search.dispose());

	async function createNote(title: string) {
		try {
			const created = (await getNouraClient().commands.execute('notes.create', {
				title,
			})) as { id?: string } | null | undefined;
			commandPalette.close();
			if (created?.id) {
				await goto(`/notes?selected=${encodeURIComponent(created.id)}`);
				toast.success(`Created “${title}”`);
			}
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not create the note',
			);
		}
	}

	async function createTask(title: string) {
		try {
			const created = (await getNouraClient().commands.execute('tasks.create', {
				title,
			})) as { id?: string } | null | undefined;
			commandPalette.close();
			if (created?.id) {
				await goto(
					`/tasks?view=all&selected=${encodeURIComponent(created.id)}`,
				);
				toast.success(`Created “${title}”`);
			}
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not create the task',
			);
		}
	}

	async function openResult(result: SearchResult) {
		const target = searchResultTarget(result);
		if (!target || !plugins.isEnabled(target.pluginId)) return;
		commandPalette.close();
		await goto(navigationHref(target));
	}

	async function openRoute(route: string) {
		commandPalette.close();
		await goto(route);
	}

	function handleKeydown(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
			event.preventDefault();
			commandPalette.toggle();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<Command.Dialog
	bind:open={commandPalette.open}
	shouldFilter={false}
	title="Search and commands"
	description="Search the workspace, navigate, or create things"
	class="max-w-xl"
>
	<Command.Input bind:value={query} placeholder="Search or type to create…" />
	<Command.List>
		{#if searching && results.length === 0}
			<Command.Loading>Searching…</Command.Loading>
		{/if}
		{#if query.trim().length === 0}
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
		{:else}
			<Command.Group heading="Actions">
				{#if plugins.isEnabled('notes')}
					<Command.Item
						value="action:new-note"
						onSelect={() => void createNote(query.trim())}
					>
						<NotePencil class="text-muted-foreground" />
						<span>New note “{query.trim()}”</span>
					</Command.Item>
				{/if}
				{#if plugins.isEnabled('tasks')}
					<Command.Item
						value="action:new-task"
						onSelect={() => void createTask(query.trim())}
					>
						<Checks class="text-muted-foreground" />
						<span>New task “{query.trim()}”</span>
					</Command.Item>
				{/if}
			</Command.Group>
			{#if visibleResults.length > 0}
				<Command.Group heading="Results">
					{#each visibleResults as result (result.relativePath)}
						{@const Icon = resultIcon(result)}
						{@const target = searchResultTarget(result)}
						<Command.Item
							value="result:{result.relativePath}"
							onSelect={() => void openResult(result)}
							class={target ? '' : 'text-muted-foreground'}
						>
							<Icon class="shrink-0 text-muted-foreground" />
							<div class="min-w-0 flex-1">
								<div class="flex items-center gap-2">
									<span class="truncate font-medium">{result.title}</span>
									{#if result.objectType}
										<Badge variant="outline" class="text-[10px] capitalize"
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
			{:else if searchError}
				<p class="px-3 py-4 text-xs text-destructive" role="alert">
					{searchError}
				</p>
			{:else if !searching}
				<p class="px-3 py-4 text-xs text-muted-foreground">
					No matches for “{query.trim()}”.
				</p>
			{/if}
		{/if}
	</Command.List>
</Command.Dialog>
