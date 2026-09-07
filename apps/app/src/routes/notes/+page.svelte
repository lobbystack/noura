<script lang="ts">
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { isPlainTextPath } from '$lib/editor/text-files';
	import { LiveProjection } from '$lib/live-refresh';
	import { plugins } from '$lib/plugins.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { browser } from '$app/environment';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { toast } from 'svelte-sonner';
	import { onMount } from 'svelte';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['list']>
	>[number];
	type RawFile =
		| Awaited<
				ReturnType<
					ReturnType<typeof getNouraClient>['files']['listNonManagedMarkdown']
				>
		  >[number]
		| { relativePath: string; title: string; parseStatus: null };

	// These lists are lookup indexes for URL-driven selection, not a
	// navigator: the sidebar file tree owns navigation now.
	let notes = $state<Note[]>([]);
	let rawFiles = $state<RawFile[]>([]);
	let loaded = $state(false);
	let selected = $state<Note | null>(null);
	let selectedRaw = $state<RawFile | null>(null);
	let appliedKey: string | null = null;
	let autofocusTitle = $state(false);
	let projection = $state.raw<LiveProjection | null>(null);
	let selectionGeneration = 0;
	let loadGeneration = 0;

	async function select(
		n: Note,
		options?: { isNew?: boolean; selectionGeneration?: number },
	) {
		if (selected?.id === n.id) return;
		if (!(await flushPendingDrafts())) return;
		if (
			options?.selectionGeneration !== undefined &&
			options.selectionGeneration !== selectionGeneration
		)
			return;
		autofocusTitle = options?.isNew ?? false;
		selected = n;
		selectedRaw = null;
		tabsStore.open(n.id, 'note', n.title);
	}

	async function selectRaw(file: RawFile, requestedGeneration?: number) {
		if (selectedRaw?.relativePath === file.relativePath) return;
		if (!(await flushPendingDrafts())) return;
		if (
			requestedGeneration !== undefined &&
			requestedGeneration !== selectionGeneration
		)
			return;
		autofocusTitle = false;
		selected = null;
		selectedRaw = file;
		tabsStore.open(`raw:${file.relativePath}`, 'markdown', file.title);
	}

	async function load(): Promise<void> {
		const generation = ++loadGeneration;
		const workspaceId = workspace.state?.workspaceId;
		const [list, raw, entries] = await Promise.all([
			getNouraClient().notes.list(),
			getNouraClient().files.listNonManagedMarkdown(),
			getNouraClient().files.list(),
		]);
		if (
			generation === loadGeneration &&
			workspace.state?.workspaceId === workspaceId
		) {
			notes = list;
			rawFiles = [
				...raw,
				...entries
					.filter(
						(entry) =>
							entry.kind === 'file' &&
							entry.parseStatus !== 'managed' &&
							isPlainTextPath(entry.relativePath) &&
							!raw.some((file) => file.relativePath === entry.relativePath),
					)
					.map((entry) => ({
						relativePath: entry.relativePath,
						title: entry.name,
						parseStatus: null,
					})),
			];
			loaded = true;
		}
	}

	// Selection is URL-driven: /notes?selected=<id> or /notes?raw=<path>.
	// Plain /notes keeps whatever is already open, like any editor surface.
	afterNavigate(() => {
		if (!browser) return;
		const params = $page.url.searchParams;
		const selectedId = params.get('selected');
		const rawPath = params.get('raw');
		const key = `${selectedId ?? ''}|${rawPath ?? ''}`;
		if (key === appliedKey) return;
		const generation = ++selectionGeneration;
		appliedKey = key;
		if (key === '|') return;
		if (selected?.id === selectedId) return;
		if (selectedRaw?.relativePath === rawPath) return;
		void (async () => {
			try {
				// Command-palette navigation can change this URL while this page is
				// already mounted. Refresh both lookup projections before resolving a
				// managed ID or raw path so external additions and moves are visible.
				if (!loaded || selectedId !== null || rawPath !== null) await load();
				if (generation !== selectionGeneration || key !== appliedKey) return;
				if (selectedId !== null) {
					const note = notes.find((entry) => entry.id === selectedId);
					if (note) {
						// Param-driven selection comes from the sidebar tree; a
						// pristine "Untitled" empty note is a fresh creation via
						// the tree context menu, so its title starts selected.
						const pristine =
							note.title === 'Untitled' && note.body.trim().length === 0;
						await select(note, {
							isNew: pristine,
							selectionGeneration: generation,
						});
					} else {
						toast.error('That note is no longer in the workspace');
					}
				} else if (rawPath !== null) {
					const file = rawFiles.find((entry) => entry.relativePath === rawPath);
					if (file) {
						await selectRaw(file, generation);
					} else {
						toast.error('That document is no longer in the workspace');
					}
				}
			} catch (error) {
				toast.error(
					error instanceof Error
						? error.message
						: 'Could not open the document',
				);
			}
		})();
	});

	onMount(() => {
		if (!browser) return;
		const coordinator = new LiveProjection({
			refresh: load,
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
		});
		projection = coordinator;
		void coordinator.start().catch(() => {});
		return () => {
			coordinator.dispose();
			if (projection === coordinator) projection = null;
		};
	});

	async function create() {
		if (!(await flushPendingDrafts())) return;
		try {
			const res = await getNouraClient().notes.create({ title: 'Untitled' });
			await load();
			if (res.value) {
				await select(res.value as Note, { isNew: true });
			}
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not create the note',
			);
		}
	}

	function handleSaved(updated: Note) {
		notes = notes.some((entry) => entry.id === updated.id)
			? notes.map((entry) => (entry.id === updated.id ? updated : entry))
			: [...notes, updated];
		selected = updated;
		tabsStore.renameObject(updated.id, updated.title);
	}

	async function handleManaged(
		object: import('@noura/workspace').WorkspaceObject,
	) {
		// The raw-file service emits this identity only after the repaired bytes
		// are durable and indexed. Transition directly instead of asking the same
		// in-flight editor to flush again through the normal navigation guard.
		selectedRaw = null;
		selected = null;
		tabsStore.open(object.id, object.type, object.title);
		if (object.type === 'task') {
			await goto(`/tasks?selected=${encodeURIComponent(object.id)}`);
			return;
		}
		if (object.type === 'project') {
			await goto(`/projects?selected=${encodeURIComponent(object.id)}`);
			return;
		}
		await load();
		const managed = notes.find((note) => note.id === object.id);
		if (managed) selected = managed;
	}
</script>

<div class="flex h-full flex-col">
	<div
		class="flex h-14 shrink-0 items-center justify-between border-b border-border px-6"
	>
		<h1 class="text-sm font-semibold">Notes</h1>
		{#if plugins.isEnabled('notes')}
			<Button size="sm" onclick={create}>
				<Plus data-icon="inline-start" />
				New note
			</Button>
		{/if}
	</div>

	<div class="min-h-0 flex-1">
		{#key selected?.id ?? `raw:${selectedRaw?.relativePath ?? ''}`}
			{#if selectedRaw}
				<RawMarkdownEditor file={selectedRaw} onmanaged={handleManaged} />
			{:else}
				<NoteEditor note={selected} onsaved={handleSaved} {autofocusTitle} />
			{/if}
		{/key}
	</div>
</div>
