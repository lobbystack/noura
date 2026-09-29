<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { isPlainTextPath } from '$lib/editor/text-files';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import { findTreeNode } from '$lib/workspace-tree';
	import type { UnmanagedFile, WorkspaceObject } from '@noura/workspace';
	import { plugins } from '$lib/plugins.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { saveBeforeLeaving } from '$lib/editor/unsaved-changes';
	import { splitFileName } from '$lib/editor/rename';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
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
	/** Stays the same when an open file is renamed, so the editor stays put. */
	let rawKey = $state(0);
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
		return {
			relativePath,
			title: splitFileName(relativePath).stem,
			parseStatus: node?.parseStatus ?? null,
		};
	}

	/** Save the open editor first; false when the user has to decide. */
	function leaveCurrent(proceed: () => void | Promise<void>) {
		return saveBeforeLeaving(proceed);
	}

	async function select(
		n: Note,
		options?: { isNew?: boolean; selectionGeneration?: number },
	) {
		if (selected?.id === n.id) return;
		const show = () => {
			if (
				options?.selectionGeneration !== undefined &&
				options.selectionGeneration !== selectionGeneration
			)
				return;
			autofocusTitle = options?.isNew ?? false;
			selected = n;
			selectedRaw = null;
			tabsStore.open(n.id, 'note', splitFileName(n.relativePath).stem);
		};
		if (await leaveCurrent(show)) show();
	}

	async function selectRaw(file: RawFile, requestedGeneration?: number) {
		if (selectedRaw?.relativePath === file.relativePath) return;
		const show = () => {
			if (
				requestedGeneration !== undefined &&
				requestedGeneration !== selectionGeneration
			)
				return;
			autofocusTitle = false;
			selected = null;
			selectedRaw = file;
			rawKey += 1;
			tabsStore.open(`raw:${file.relativePath}`, 'markdown', file.title);
		};
		if (await leaveCurrent(show)) show();
	}

	// Selection is URL-driven: /notes?selected=<id> or /notes?raw=<path>.
	// Plain /notes keeps whatever is already open, like any editor surface.
	afterNavigate(() => {
		const params = page.url.searchParams;
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
						// the tree context menu, so its name starts selected.
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
						toast.error('That file is no longer in the workspace');
					}
				}
			} catch (error) {
				toast.error(
					error instanceof Error ? error.message : 'Couldn’t open the file',
				);
			}
		})();
	});

	async function create() {
		const run = async () => {
			try {
				const res = await getNouraClient().notes.create({ title: 'Untitled' });
				if (res.value) {
					await select(res.value as Note, { isNew: true });
				}
			} catch (error) {
				toast.error(
					error instanceof Error ? error.message : 'Couldn’t create the note',
				);
			}
		};
		if (await leaveCurrent(run)) await run();
	}

	function handleSaved(updated: Note) {
		selected = updated;
		tabsStore.renameObject(
			updated.id,
			splitFileName(updated.relativePath).stem,
		);
	}

	async function handleRawRenamed(relativePath: string) {
		const previous = selectedRaw;
		if (!previous) return;
		const title = splitFileName(relativePath).stem;
		selectedRaw = { ...previous, relativePath, title };
		tabsStore.retarget(
			`raw:${previous.relativePath}`,
			`raw:${relativePath}`,
			title,
		);
		// Keep the address in step without reopening the editor.
		appliedKey = `|${relativePath}`;
		await goto(`?raw=${encodeURIComponent(relativePath)}`, {
			replaceState: true,
			keepFocus: true,
			noScroll: true,
		});
	}

	async function handleManaged(object: WorkspaceObject) {
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

	<div class="flex min-h-0 flex-1 flex-col">
		{#key selected?.id ?? `raw:${selectedRaw ? rawKey : ''}`}
			{#if selectedRaw}
				<RawMarkdownEditor
					file={selectedRaw}
					onmanaged={handleManaged}
					onrenamed={handleRawRenamed}
				/>
			{:else}
				<NoteEditor note={selected} onsaved={handleSaved} {autofocusTitle} />
			{/if}
		{/key}
	</div>
</div>
