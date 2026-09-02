<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';
	import { cn } from '$lib/utils';
	import PageHeader from '$lib/components/page-header.svelte';
	import NoteEditor from '$lib/components/note-editor.svelte';
	import RawMarkdownEditor from '$lib/components/raw-markdown-editor.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Plus from 'phosphor-svelte/lib/Plus';
	import { browser } from '$app/environment';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	type Note = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['notes']['list']>
	>[number];

	let notes = $state<Note[]>([]);
	let rawFiles = $state<
		Awaited<
			ReturnType<
				ReturnType<typeof getNouraClient>['files']['listNonManagedMarkdown']
			>
		>
	>([]);
	let loading = $state(true);
	let selected = $state<Note | null>(null);
	let selectedRaw = $state<(typeof rawFiles)[number] | null>(null);
	async function select(n: Note) {
		if (selected?.id !== n.id && !(await flushPendingDrafts())) return;
		selected = n;
		selectedRaw = null;
		tabsStore.open(n.id, 'note', n.title);
	}

	async function selectRaw(file: (typeof rawFiles)[number]) {
		if (!(await flushPendingDrafts())) return;
		selected = null;
		selectedRaw = file;
		tabsStore.open(`raw:${file.relativePath}`, 'markdown', file.title);
	}

	async function load() {
		try {
			loading = true;
			const [list, raw] = await Promise.all([
				getNouraClient().notes.list(),
				getNouraClient().files.listNonManagedMarkdown(),
			]);
			notes = list;
			rawFiles = raw;
			// Pre-select first note if one exists and nothing is selected yet
			if (!selected && list.length > 0) await select(list[0]);
		} finally {
			loading = false;
		}
	}

	async function create() {
		if (!(await flushPendingDrafts())) return;
		const res = await getNouraClient().notes.create({ title: 'Untitled' });
		await load();
		if (res.value) await select(res.value as Note);
	}

	function handleSaved(updated: Note) {
		// Refresh the list so the sidebar reflects renames
		notes = notes.map((n) => (n.id === updated.id ? updated : n));
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

		const [list, raw] = await Promise.all([
			getNouraClient().notes.list(),
			getNouraClient().files.listNonManagedMarkdown(),
		]);
		notes = list;
		rawFiles = raw;
		const managed = list.find((note) => note.id === object.id);
		if (managed) selected = managed;
	}

	onMount(() => {
		if (browser) load();
	});
</script>

<div class="flex h-screen">
	<aside
		class="flex w-72 shrink-0 flex-col border-r border-border bg-background"
	>
		<PageHeader
			title="Notes"
			description={`${notes.length + rawFiles.length} documents`}
		>
			{#snippet actions()}
				<Button size="sm" onclick={create}>
					<Plus data-icon="inline-start" />
					New note
				</Button>
			{/snippet}
		</PageHeader>

		{#if loading}
			<div class="flex flex-col gap-1 p-2">
				{#each [0, 1, 2, 3] as i (i)}
					<Skeleton class="h-10 w-full" />
				{/each}
			</div>
		{:else if notes.length === 0 && rawFiles.length === 0}
			<EmptyState
				icon={NotePencil}
				title="No notes"
				description="Capture your first thought."
				actionLabel="Create note"
				onAction={create}
			/>
		{:else}
			<div class="flex-1 overflow-y-auto">
				<div class="divide-y divide-border/60">
					{#each notes as n (n.id)}
						<button
							class={cn(
								'w-full px-4 py-3 text-left hover:bg-muted/50',
								selected?.id === n.id && 'bg-muted/60',
							)}
							onclick={() => void select(n)}
						>
							<div class="flex items-center gap-2">
								<NotePencil
									class={cn(
										'size-4 shrink-0',
										selected?.id === n.id
											? 'text-foreground'
											: 'text-muted-foreground',
									)}
								/>
								<span
									class={cn(
										'truncate text-sm',
										selected?.id === n.id
											? 'font-semibold text-foreground'
											: 'font-medium',
									)}
								>
									{n.title}
								</span>
							</div>
							{#if n.body}
								<p
									class="mt-0.5 line-clamp-1 pl-6 text-xs text-muted-foreground"
								>
									{n.body}
								</p>
							{/if}
						</button>
					{/each}
					{#each rawFiles as file (file.relativePath)}
						<button
							class={cn(
								'w-full px-4 py-3 text-left hover:bg-muted/50',
								selectedRaw?.relativePath === file.relativePath &&
									'bg-muted/60',
							)}
							onclick={() => void selectRaw(file)}
						>
							<div class="flex items-center gap-2">
								<NotePencil /><span
									class="min-w-0 flex-1 truncate text-sm font-medium"
									>{file.title}</span
								><Badge variant="secondary"
									>{file.parseStatus === 'malformed'
										? 'Needs repair'
										: 'Markdown'}</Badge
								>
							</div>
							<p class="mt-0.5 truncate pl-6 text-xs text-muted-foreground">
								{file.relativePath}
							</p>
						</button>
					{/each}
				</div>
			</div>
		{/if}
	</aside>

	<main class="flex min-w-0 flex-1 flex-col">
		{#key selected?.id ?? selectedRaw?.relativePath}
			{#if selectedRaw}
				<RawMarkdownEditor file={selectedRaw} onmanaged={handleManaged} />
			{:else}
				<NoteEditor note={selected} onsaved={handleSaved} />
			{/if}
		{/key}
	</main>
</div>
done
