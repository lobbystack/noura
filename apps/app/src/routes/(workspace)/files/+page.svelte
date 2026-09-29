<script lang="ts">
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { isPlainTextPath } from '$lib/editor/text-files';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import { findTreeNode } from '$lib/workspace-tree';
	import type { UnmanagedFile } from '@noura/workspace';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import { browser } from '$app/environment';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { toast } from 'svelte-sonner';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['get']>
	>;
	type RawFile = {
		relativePath: string;
		title: string;
		parseStatus: UnmanagedFile['parseStatus'] | null;
	};

	let selected = $state<Note | null>(null);
	let selectedRaw = $state<RawFile | null>(null);
	let appliedKey: string | null = null;
	let autofocusTitle = $state(false);
	let selectionGeneration = 0;

	// Opening a document reads only that document. The sidebar tree already
	// holds every path and parse status, so there is nothing to list first.
	async function findNote(id: string): Promise<Note | null> {
		const client = getNouraClient();
		try {
			return await client.notes.get(id);
		} catch {
			// The index may not have seen an external change yet. Listing the
			// files reconciles it, then the note resolves if it exists.
			await client.files.list();
			return await client.notes.get(id).catch(() => null);
		}
	}

	function rawFileAt(relativePath: string): RawFile | null {
		if (!/\.md$/i.test(relativePath) && !isPlainTextPath(relativePath))
			return null;
		const node = findTreeNode(workspaceTree.tree, relativePath);
		const name = relativePath.split('/').pop() ?? relativePath;
		return {
			relativePath,
			title: name.replace(/\.md$/i, ''),
			parseStatus: node?.parseStatus ?? null,
		};
	}

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

	// Selection is URL-driven: /files?selected=<id> or /files?raw=<path>.
	// Plain /files keeps whatever is already open, like any editor surface.
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
				if (selectedId !== null) {
					const note = await findNote(selectedId);
					if (generation !== selectionGeneration || key !== appliedKey) return;
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
					const file = rawFileAt(rawPath);
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

	function handleSaved(updated: Note) {
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
		const managed = await findNote(object.id);
		if (managed) selected = managed;
	}
</script>

<div class="flex h-full flex-col">
	<div class="flex min-h-0 flex-1 flex-col">
		{#key selected?.id ?? `raw:${selectedRaw?.relativePath ?? ''}`}
			{#if selectedRaw}
				<RawMarkdownEditor file={selectedRaw} onmanaged={handleManaged} />
			{:else}
				<NoteEditor note={selected} onsaved={handleSaved} {autofocusTitle} />
			{/if}
		{/key}
	</div>
</div>
