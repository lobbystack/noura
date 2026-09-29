<script lang="ts">
	import { getNouraClient, getPluginRuntime } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import { getAppPlatform } from '$lib/platform';
	import { getBrowserWorkspace } from '$lib/browser-workspace';
	import { isPlainTextPath } from '$lib/editor/text-files';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import { findTreeNode, NOT_DOWNLOADED_MESSAGE } from '$lib/workspace-tree';
	import {
		isCoreError,
		type UnmanagedFile,
		type WorkspaceObject,
	} from '@noura/workspace';
	import { tabHref, tabsStore } from '$lib/tabs.svelte';
	import { saveBeforeLeaving } from '$lib/editor/unsaved-changes';
	import { splitFileName } from '$lib/editor/rename';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import { onMount } from 'svelte';
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

	const web = getAppPlatform() === 'web';
	let selected = $state<Note | null>(null);
	let selectedRaw = $state<RawFile | null>(null);
	/** Stays the same when an open file is renamed, so the editor stays put. */
	let rawKey = $state(0);
	let appliedKey: string | null = null;

	// A document shows only while its tab is open: closing the tab, or
	// trashing the file from the tree, closes it here too.
	const hasTab = (objectId: string) =>
		tabsStore.tabs.some((tab) => tab.objectId === objectId);
	const shownNote = $derived(selected && hasTab(selected.id) ? selected : null);
	const shownRaw = $derived(
		selectedRaw && hasTab(`raw:${selectedRaw.relativePath}`)
			? selectedRaw
			: null,
	);
	let autofocusTitle = $state(false);
	let selectionGeneration = 0;
	// Editors reopen when the sync plugin adds or removes collaboration.
	const collaborationSlot = getPluginRuntime().collaboration;

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

	/**
	 * A cloud placeholder opens only once the system downloads it. Tabs,
	 * links and search can still point at one, so check before opening.
	 */
	function notDownloaded(relativePath: string) {
		if (!findTreeNode(workspaceTree.tree, relativePath)?.notDownloaded)
			return false;
		toast(NOT_DOWNLOADED_MESSAGE);
		return true;
	}

	/** Save the open editor first; false when the user has to decide. */
	function leaveCurrent(proceed: () => void | Promise<void>) {
		return saveBeforeLeaving(proceed);
	}

	async function select(
		n: Note,
		options?: { isNew?: boolean; selectionGeneration?: number },
	) {
		if (shownNote?.id === n.id) return;
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
		if (shownRaw?.relativePath === file.relativePath) return;
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

	/**
	 * Plain /files keeps whatever is already open, like any editor surface.
	 * With nothing open it shows the active tab's document, so the page and
	 * the highlighted tab agree.
	 */
	function showActiveTab() {
		if (shownNote || shownRaw) return;
		const active = tabsStore.active;
		if (!active) return;
		const href = tabHref(active);
		if (new URL(href, page.url).pathname === page.url.pathname)
			void goto(href, { replaceState: true });
	}

	// Selection is URL-driven: /files?selected=<id> or /files?raw=<path>.
	// The page mounts once the workspace is ready, after the navigation that
	// opened the app, so it applies the address on mount too.
	onMount(() => applyAddress());
	afterNavigate(() => applyAddress());

	function applyAddress() {
		const params = page.url.searchParams;
		const selectedId = params.get('selected');
		const rawPath = params.get('raw');
		const key = `${selectedId ?? ''}|${rawPath ?? ''}`;
		if (key === appliedKey) return;
		const generation = ++selectionGeneration;
		appliedKey = key;
		if (key === '|') {
			showActiveTab();
			return;
		}
		if (shownNote?.id === selectedId) return;
		if (shownRaw?.relativePath === rawPath) return;
		void (async () => {
			try {
				if (selectedId !== null) {
					const note = await findNote(selectedId);
					if (generation !== selectionGeneration || key !== appliedKey) return;
					if (note && notDownloaded(note.relativePath)) return;
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
					if (file && notDownloaded(file.relativePath)) return;
					if (file) {
						await selectRaw(file, generation);
					} else {
						toast.error('That file is no longer in the workspace');
					}
				}
			} catch (error) {
				if (isCoreError(error) && error.code === 'file_not_downloaded')
					toast(NOT_DOWNLOADED_MESSAGE);
				else
					toast.error(
						error instanceof Error ? error.message : 'Couldn’t open the file',
					);
			}
		})();
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
	<div class="flex min-h-0 flex-1 flex-col">
		{#key `${$collaborationSlot.generation}:${shownNote?.id ?? `raw:${shownRaw ? rawKey : ''}`}`}
			{#if shownRaw}
				<RawMarkdownEditor
					file={shownRaw}
					onmanaged={handleManaged}
					onrenamed={handleRawRenamed}
				/>
			{:else}
				<NoteEditor note={shownNote} onsaved={handleSaved} {autofocusTitle} />
			{/if}
		{/key}
	</div>
	{#if web && shownNote && plugins.isEnabled('sync')}
		<!-- Browser sync sends attachments per note; the desktop app syncs files. -->
		{#await import('$lib/components/browser-attachments.svelte') then { default: Attachments }}
			{#key shownNote.id}
				<div class="shrink-0 border-t border-border px-6 py-4">
					<Attachments
						noteId={shownNote.id}
						notePath={shownNote.relativePath}
						workspaceFiles={getBrowserWorkspace().files}
					/>
				</div>
			{/key}
		{/await}
	{/if}
</div>
