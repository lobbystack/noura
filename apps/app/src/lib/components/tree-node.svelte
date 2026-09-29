<script lang="ts">
	import {
		SidebarMenuButton,
		SidebarMenuItem,
	} from '$lib/components/ui/sidebar';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import {
		displayName,
		treeTargetFor,
		type VisibleTreeRow,
		type WorkspaceTreeNode,
	} from '$lib/workspace-tree';
	import { cn } from '$lib/utils.js';
	import type { Attachment } from 'svelte/attachments';
	import Checks from 'phosphor-svelte/lib/Checks';
	import CaretRight from 'phosphor-svelte/lib/CaretRight';
	import File from 'phosphor-svelte/lib/File';
	import FilePdf from 'phosphor-svelte/lib/FilePdf';
	import FileText from 'phosphor-svelte/lib/FileText';
	import Folder from 'phosphor-svelte/lib/Folder';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Kanban from 'phosphor-svelte/lib/Kanban';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';

	let {
		row,
		expanded,
		selected,
		active,
		tabbable,
		renaming,
		dropTarget,
		dragging,
		onrename,
		oncancelrename,
	}: {
		row: VisibleTreeRow;
		expanded: boolean;
		/** The row with keyboard focus. */
		selected: boolean;
		/** The row whose document is open. */
		active: boolean;
		tabbable: boolean;
		renaming: boolean;
		dropTarget: boolean;
		dragging: boolean;
		onrename: (name: string) => void;
		oncancelrename: () => void;
	} = $props();

	const node = $derived(row.node);
	const opensHere = $derived(
		node.kind === 'folder' || treeTargetFor(node).route !== null,
	);
	const indent = $derived(`padding-left: ${8 + row.depth * 12}px`);

	function fileIcon(file: WorkspaceTreeNode) {
		if (file.parseStatus === 'managed' && file.objectType === 'note')
			return NotePencil;
		if (file.parseStatus === 'managed' && file.objectType === 'task')
			return Checks;
		if (file.parseStatus === 'managed' && file.objectType === 'project')
			return Kanban;
		if (/\.pdf$/i.test(file.name)) return FilePdf;
		return file.parseStatus === null ? File : FileText;
	}

	const Icon = $derived(
		node.kind === 'folder' ? (expanded ? FolderOpen : Folder) : fileIcon(node),
	);

	// Select the name without its extension so typing replaces it, like Finder.
	const focusName: Attachment<HTMLInputElement> = (input) => {
		finished = false;
		input.focus();
		const dot = node.kind === 'file' ? input.value.lastIndexOf('.') : -1;
		input.setSelectionRange(0, dot > 0 ? dot : input.value.length);
	};

	let finished = false;
	function finish(commit: boolean, value: string) {
		if (finished) return;
		finished = true;
		if (commit) onrename(value);
		else oncancelrename();
	}
</script>

<SidebarMenuItem role="none">
	{#if renaming}
		<div class="flex h-7 items-center gap-2 pr-2" style={indent}>
			<Icon class="size-4 shrink-0 text-muted-foreground" />
			<Input
				value={displayName(node)}
				aria-label="New name for {displayName(node)}"
				class="h-6 px-1.5 text-xs"
				spellcheck="false"
				autocomplete="off"
				{@attach focusName}
				onkeydown={(event) => {
					event.stopPropagation();
					if (event.key === 'Enter') {
						event.preventDefault();
						finish(true, event.currentTarget.value);
					} else if (event.key === 'Escape') {
						event.preventDefault();
						finish(false, '');
					}
				}}
				onblur={(event) => finish(true, event.currentTarget.value)}
			/>
		</div>
	{:else}
		{#if node.notDownloaded}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						{@render button(props)}
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content side="right">Not downloaded</Tooltip.Content>
			</Tooltip.Root>
		{:else}
			{@render button({})}
		{/if}
	{/if}
</SidebarMenuItem>

{#snippet button(props: Record<string, unknown>)}
	<SidebarMenuButton
		{...props}
		role="treeitem"
		size="sm"
		isActive={active}
		tabindex={tabbable ? 0 : -1}
		aria-level={row.depth + 1}
		aria-setsize={row.siblingCount}
		aria-posinset={row.position}
		aria-expanded={node.kind === 'folder' ? expanded : undefined}
		aria-selected={selected}
		data-tree-path={node.relativePath}
		class={cn(
			'justify-start text-left select-none',
			selected && !active && 'bg-sidebar-accent/50',
			(!opensHere || node.notDownloaded) && 'text-muted-foreground',
			dropTarget && 'bg-sidebar-accent ring-2 ring-sidebar-ring',
			dragging && 'opacity-50',
		)}
		style={indent}
	>
		<Icon class="shrink-0 text-muted-foreground" />
		<span class="truncate">{displayName(node)}</span>
		{#if node.notDownloaded}<span class="sr-only">, not downloaded</span>{/if}
		{#if node.kind === 'folder'}
			<CaretRight
				class={cn(
					'ml-auto shrink-0 text-muted-foreground transition-transform',
					expanded && 'rotate-90',
				)}
			/>
		{/if}
	</SidebarMenuButton>
{/snippet}
