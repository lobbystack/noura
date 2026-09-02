<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { toast } from 'svelte-sonner';
	import * as ContextMenu from '$lib/components/ui/context-menu';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Field from '$lib/components/ui/field';
	import {
		SidebarMenuButton,
		SidebarMenuItem,
	} from '$lib/components/ui/sidebar';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import {
		collectFilePaths,
		collectFolderNames,
		nextUntitledPath,
		treeTargetFor,
		type WorkspaceTreeNode,
	} from '$lib/workspace-tree';
	import { plugins } from '$lib/plugins.svelte';
	import { getNouraClient } from '$lib/state.svelte';
	import { cn } from '$lib/utils.js';
	import Checks from 'phosphor-svelte/lib/Checks';
	import CaretRight from 'phosphor-svelte/lib/CaretRight';
	import File from 'phosphor-svelte/lib/File';
	import FileText from 'phosphor-svelte/lib/FileText';
	import Folder from 'phosphor-svelte/lib/Folder';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Self from './tree-node.svelte';

	let { node, depth = 0 }: { node: WorkspaceTreeNode; depth?: number } =
		$props();

	const isOpen = $derived(workspaceTree.isExpanded(node.relativePath));
	const target = $derived(
		node.kind === 'file'
			? treeTargetFor(node)
			: { route: null as string | null, query: {} as Record<string, string> },
	);
	const isActive = $derived.by(() => {
		if (!target.route || !$page.url.pathname.startsWith(target.route)) {
			return false;
		}
		if (target.query.selected) {
			return $page.url.searchParams.get('selected') === target.query.selected;
		}
		if (target.query.raw) {
			return $page.url.searchParams.get('raw') === node.relativePath;
		}
		return false;
	});

	function fileIcon(node: WorkspaceTreeNode) {
		if (node.parseStatus === 'managed' && node.objectType === 'note') {
			return NotePencil;
		}
		if (node.parseStatus === 'managed' && node.objectType === 'task') {
			return Checks;
		}
		return node.parseStatus === null ? File : FileText;
	}

	function toggle() {
		workspaceTree.toggle(node.relativePath);
	}

	async function openTarget() {
		if (!target.route) return;
		const params = new URLSearchParams(target.query);
		await goto(`${target.route}?${params.toString()}`);
	}

	async function newNoteHere() {
		if (!plugins.isEnabled('notes')) return;
		const path = nextUntitledPath(
			collectFilePaths(workspaceTree.tree),
			node.relativePath,
		);
		try {
			const result = await getNouraClient().notes.create({
				title: 'Untitled',
				relativePath: path,
			});
			await workspaceTree.refresh();
			workspaceTree.expandTo(node.relativePath);
			if (result.value) {
				await goto(`/notes?selected=${encodeURIComponent(result.value.id)}`);
			}
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not create the note',
			);
		}
	}

	let newFolderOpen = $state(false);
	let newFolderName = $state('');
	async function createFolder() {
		const name = newFolderName.trim();
		if (name.length === 0) return;
		if (name.includes('/')) {
			toast.error('Folder names cannot contain slashes');
			return;
		}
		const taken = collectFolderNames(workspaceTree.tree, node.relativePath);
		if (taken.has(name)) {
			toast.error(`A folder named "${name}" already exists here`);
			return;
		}
		const relativePath =
			node.relativePath.length > 0 ? `${node.relativePath}/${name}` : name;
		try {
			await getNouraClient().folders.create({ relativePath });
			newFolderOpen = false;
			newFolderName = '';
			await workspaceTree.refresh();
			workspaceTree.expandTo(node.relativePath);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not create the folder',
			);
		}
	}

	function revealInFileManager(): (() => void) | null {
		if (!node.objectId || !node.objectType) return null;
		const client = getNouraClient();
		const service =
			node.objectType === 'note'
				? client.notes
				: node.objectType === 'task'
					? client.tasks
					: node.objectType === 'project'
						? client.projects
						: null;
		if (!service) return null;
		const objectId = node.objectId;
		return () => {
			void service.showInFolder(objectId);
		};
	}

	const reveal = $derived(revealInFileManager());
</script>

{#if node.kind === 'folder'}
	<SidebarMenuItem>
		<ContextMenu.Root>
			<ContextMenu.Trigger>
				{#snippet child(trigger)}
					<SidebarMenuButton
						{...trigger.props}
						class={cn(
							trigger.props.class as string | undefined,
							'justify-start text-left',
						)}
						style="padding-left: {8 + depth * 12}px"
						aria-expanded={isOpen}
						onclick={toggle}
					>
						{#if isOpen}
							<FolderOpen class="shrink-0 text-muted-foreground" />
						{:else}
							<Folder class="shrink-0 text-muted-foreground" />
						{/if}
						<span class="truncate">{node.name}</span>
						<CaretRight
							class={cn(
								'ml-auto shrink-0 text-muted-foreground transition-transform',
								isOpen && 'rotate-90',
							)}
						/>
					</SidebarMenuButton>
				{/snippet}
			</ContextMenu.Trigger>
			<ContextMenu.Content>
				{#if plugins.isEnabled('notes')}
					<ContextMenu.Item onclick={() => void newNoteHere()}>
						New note here
					</ContextMenu.Item>
				{/if}
				<ContextMenu.Item onclick={() => (newFolderOpen = true)}>
					New folder…
				</ContextMenu.Item>
			</ContextMenu.Content>
		</ContextMenu.Root>
		{#if isOpen}
			<ul class="flex flex-col">
				{#each node.children as childNode (childNode.relativePath)}
					<Self node={childNode} depth={depth + 1} />
				{/each}
			</ul>
		{/if}
		<Dialog.Root bind:open={newFolderOpen}>
			<Dialog.Content class="sm:max-w-sm">
				<Dialog.Header>
					<Dialog.Title>New folder</Dialog.Title>
					<Dialog.Description>
						Created inside {node.relativePath || 'the workspace root'}.
					</Dialog.Description>
				</Dialog.Header>
				<Field.Field>
					<Field.FieldLabel for="new-folder-name">Name</Field.FieldLabel>
					<Input
						id="new-folder-name"
						bind:value={newFolderName}
						placeholder="e.g. meeting-notes"
						onkeydown={(event) => {
							if (event.key === 'Enter') {
								event.preventDefault();
								void createFolder();
							}
						}}
					/>
				</Field.Field>
				<Dialog.Footer>
					<Button
						variant="outline"
						size="sm"
						onclick={() => (newFolderOpen = false)}
					>
						Cancel
					</Button>
					<Button size="sm" onclick={() => void createFolder()}>Create</Button>
				</Dialog.Footer>
			</Dialog.Content>
		</Dialog.Root>
	</SidebarMenuItem>
{:else}
	{@const FileIcon = fileIcon(node)}
	<SidebarMenuItem>
		<ContextMenu.Root>
			<ContextMenu.Trigger>
				{#snippet child(trigger)}
					{#if target.route}
						<SidebarMenuButton
							{...trigger.props}
							{isActive}
							size="sm"
							class={cn(
								trigger.props.class as string | undefined,
								'justify-start text-left',
							)}
							style="padding-left: {8 + depth * 12}px"
							onclick={() => void openTarget()}
						>
							<FileIcon class="shrink-0 text-muted-foreground"></FileIcon>
							<span class="truncate">{node.name}</span>
						</SidebarMenuButton>
					{:else}
						<SidebarMenuButton
							{...trigger.props}
							size="sm"
							class={cn(
								trigger.props.class as string | undefined,
								'justify-start text-left text-muted-foreground',
							)}
							style="padding-left: {8 + depth * 12}px"
							title={node.name}
							aria-disabled="true"
						>
							<FileIcon class="shrink-0"></FileIcon>
							<span class="truncate">{node.name}</span>
						</SidebarMenuButton>
					{/if}
				{/snippet}
			</ContextMenu.Trigger>
			{#if reveal}
				<ContextMenu.Content>
					<ContextMenu.Item onclick={reveal}>Show in Finder</ContextMenu.Item>
				</ContextMenu.Content>
			{/if}
		</ContextMenu.Root>
	</SidebarMenuItem>
{/if}
