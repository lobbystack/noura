<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { browser } from '$app/environment';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { toast } from 'svelte-sonner';
	import { onMount } from 'svelte';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['list']>
	>[number];
	type RawFile = Awaited<
		ReturnType<
			ReturnType<typeof getNouraClient>['files']['listNonManagedMarkdown']
		>
	>[number];

	// These lists are lookup indexes for URL-driven selection, not a
	// navigator: the sidebar file tree owns navigation now.
	let notes = $state<Note[]>([]);
	let rawFiles = $state<RawFile[]>([]);
	let loaded = $state(false);
	let selected = $state<Note | null>(null);
	let selectedRaw = $state<RawFile | null>(null);
	let appliedKey = $state<string | null>(null);
	let autofocusTitle = $state(false);

	async function select(n: Note, options?: { isNew?: boolean }) {
		if (selected?.id === n.id) return;
		if (!(await flushPendingDrafts())) return;
		autofocusTitle = options?.isNew ?? false;
		selected = n;
		selectedRaw = null;
		tabsStore.open(n.id, 'note', n.title);
	}

	async function selectRaw(file: RawFile) {
		if (selectedRaw?.relativePath === file.relativePath) return;
		if (!(await flushPendingDrafts())) return;
		autofocusTitle = false;
		selected = null;
		selectedRaw = file;
		tabsStore.open(`raw:${file.relativePath}`, 'markdown', file.title);
	}

	async function load(): Promise<void> {
		const [list, raw] = await Promise.all([
			getNouraClient().notes.list(),
			getNouraClient().files.listNonManagedMarkdown(),
		]);
		notes = list;
		rawFiles = raw;
		loaded = true;
	}

	// Selection is URL-driven: /notes?selected=<id> or /notes?raw=<path>.
	// Plain /notes keeps whatever is already open, like any editor surface.
	$effect(() => {
		if (!browser) return;
		const params = $page.url.searchParams;
		const selectedId = params.get('selected');
		const rawPath = params.get('raw');
		const key = `${selectedId ?? ''}|${rawPath ?? ''}`;
		if (key === appliedKey || key === '|') return;
		appliedKey = key;
		if (selected?.id === selectedId) return;
		if (selectedRaw?.relativePath === rawPath) return;
		void (async () => {
			try {
				if (!loaded) await load();
				if (selectedId !== null) {
					const note = notes.find((entry) => entry.id === selectedId);
					if (note) {
						// Param-driven selection comes from the sidebar tree; a
						// pristine "Untitled" empty note is a fresh creation via
						// the tree context menu, so its title starts selected.
						const pristine =
							note.title === 'Untitled' && note.body.trim().length === 0;
						await select(note, { isNew: pristine });
					} else {
						toast.error('That note is no longer in the workspace');
					}
				} else if (rawPath !== null) {
					const file = rawFiles.find((entry) => entry.relativePath === rawPath);
					if (file) {
						await selectRaw(file);
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
		if (browser) void load();
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
