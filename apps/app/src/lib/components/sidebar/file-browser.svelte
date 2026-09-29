<script lang="ts">
	import { page } from '$app/state';
	import { tick, untrack } from 'svelte';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import * as ContextMenu from '$lib/components/ui/context-menu/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import TreeNode from '$lib/components/tree-node.svelte';
	import WorkspaceTreeRefresh from './workspace-tree-refresh.svelte';
	import {
		canMoveInto,
		displayName,
		findTreeNode,
		openDocumentPath,
		parentPathOf,
		treeTargetFor,
		typeAheadIndex,
		visibleTreeRows,
		type WorkspaceTreeNode,
	} from '$lib/workspace-tree';
	import { treeKeyAction } from '$lib/tree-keyboard';
	import {
		createFolder,
		createNote,
		duplicateTreeNode,
		moveTreeNode,
		openTreeNode,
		openWithDefaultApp,
		renameTreeNode,
		revealTreeNode,
		trashTreeNode,
	} from '$lib/file-actions';
	import { hostOs, revealLabel, shortcutLabel } from '$lib/host-os';
	import { hostCapabilities } from '$lib/host-capabilities.svelte';
	import ArrowsInLineVertical from 'phosphor-svelte/lib/ArrowsInLineVertical';
	import FolderSimplePlus from 'phosphor-svelte/lib/FolderSimplePlus';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';

	const reveal = revealLabel();
	const capabilities = $derived(hostCapabilities.current);
	const trashShortcut =
		hostOs() === 'mac' ? shortcutLabel(['Mod', '⌫']) : 'Del';

	let treeElement = $state<HTMLUListElement | null>(null);
	let areaElement = $state<HTMLElement | null>(null);

	const rows = $derived(
		visibleTreeRows(workspaceTree.tree, workspaceTree.expanded),
	);
	const activePath = $derived(openDocumentPath(page.url, workspaceTree.tree));
	const focusIndex = $derived.by(() => {
		const wanted = workspaceTree.selectedPath ?? activePath;
		return wanted === null
			? -1
			: rows.findIndex((row) => row.node.relativePath === wanted);
	});
	const tabbableIndex = $derived(focusIndex === -1 ? 0 : focusIndex);

	// Reveal the open document: open its folders, select it, scroll to it.
	$effect(() => {
		const path = activePath;
		if (!path) return;
		untrack(() => {
			workspaceTree.expandTo(path);
			workspaceTree.selectedPath = path;
		});
		void tick().then(() =>
			rowElement(path)?.scrollIntoView({ block: 'nearest' }),
		);
	});

	function rowElement(path: string): HTMLElement | null {
		return (
			treeElement?.querySelector<HTMLElement>(
				`[data-tree-path="${CSS.escape(path)}"]`,
			) ?? null
		);
	}

	function nodeAt(event: Event): WorkspaceTreeNode | null {
		const target = event.target instanceof Element ? event.target : null;
		const path = target
			?.closest('[data-tree-path]')
			?.getAttribute('data-tree-path');
		return path == null ? null : findTreeNode(workspaceTree.tree, path);
	}

	function opensHere(node: WorkspaceTreeNode) {
		return treeTargetFor(node).route !== null;
	}

	function focusPath(path: string | null) {
		if (path === null) return;
		workspaceTree.selectedPath = path;
		void tick().then(() => rowElement(path)?.focus());
	}

	function activate(node: WorkspaceTreeNode, keep = false) {
		if (node.kind === 'folder') workspaceTree.toggle(node.relativePath);
		else void openTreeNode(node, { keep });
	}

	function startRename(node: WorkspaceTreeNode) {
		workspaceTree.selectedPath = node.relativePath;
		workspaceTree.renamingPath = node.relativePath;
	}

	async function finishRename(node: WorkspaceTreeNode, name: string) {
		workspaceTree.renamingPath = null;
		await renameTreeNode(node, name);
		focusPath(workspaceTree.selectedPath);
	}

	function cancelRename(node: WorkspaceTreeNode) {
		workspaceTree.renamingPath = null;
		focusPath(node.relativePath);
	}

	let trashCandidate = $state<WorkspaceTreeNode | null>(null);

	function requestTrash(node: WorkspaceTreeNode) {
		// Only folders ask first: a file goes to the system trash, where it
		// can be restored, and a toast says so.
		if (node.kind === 'folder') trashCandidate = node;
		else void trash(node);
	}

	async function trash(node: WorkspaceTreeNode) {
		const index = rows.findIndex(
			(row) => row.node.relativePath === node.relativePath,
		);
		const next =
			rows
				.slice(index + 1)
				.find(
					(row) => !row.node.relativePath.startsWith(`${node.relativePath}/`),
				) ?? rows[index - 1];
		await trashTreeNode(node);
		focusPath(next?.node.relativePath ?? null);
	}

	// Type-ahead: letters typed in quick succession jump to a matching name.
	let typed = '';
	let typedAt = 0;

	function handleKeydown(event: KeyboardEvent) {
		if (workspaceTree.renamingPath || drag) return;
		const action = treeKeyAction(
			event,
			rows,
			focusIndex,
			workspaceTree.expanded,
			activePath,
		);
		if (!action) {
			if (
				event.key.length === 1 &&
				event.key !== ' ' &&
				!event.metaKey &&
				!event.ctrlKey &&
				!event.altKey
			) {
				const now = Date.now();
				typed = now - typedAt < 700 ? typed + event.key : event.key;
				typedAt = now;
				const match = typeAheadIndex(rows, focusIndex, typed);
				if (match !== -1) {
					event.preventDefault();
					focusPath(rows[match]!.node.relativePath);
				}
			}
			return;
		}
		event.preventDefault();
		const node = rows[action.index]?.node;
		if (!node) return;
		switch (action.type) {
			case 'focus':
				focusPath(node.relativePath);
				break;
			case 'expand':
				workspaceTree.setExpanded(node.relativePath, true);
				break;
			case 'collapse':
				workspaceTree.setExpanded(node.relativePath, false);
				break;
			case 'activate':
				workspaceTree.selectedPath = node.relativePath;
				if (node.kind === 'file' && !opensHere(node))
					void openWithDefaultApp(node);
				else activate(node);
				break;
			case 'rename':
				startRename(node);
				break;
			case 'trash':
				requestTrash(node);
				break;
			case 'menu': {
				const rect = rowElement(node.relativePath)?.getBoundingClientRect();
				rowElement(node.relativePath)?.dispatchEvent(
					new MouseEvent('contextmenu', {
						bubbles: true,
						cancelable: true,
						clientX: (rect?.left ?? 0) + 24,
						clientY: rect?.bottom ?? 0,
					}),
				);
				break;
			}
		}
	}

	function handleClick(event: MouseEvent) {
		if (suppressClick) {
			suppressClick = false;
			return;
		}
		const node = nodeAt(event);
		if (!node) return;
		workspaceTree.selectedPath = node.relativePath;
		// Files noura can't show only get selected; a double-click or Enter
		// opens them in their default app.
		if (node.kind === 'file' && !opensHere(node)) return;
		activate(node, event.metaKey || event.ctrlKey);
	}

	function handleDoubleClick(event: MouseEvent) {
		const node = nodeAt(event);
		if (!node || node.kind !== 'file') return;
		if (opensHere(node)) void openTreeNode(node, { keep: true });
		else void openWithDefaultApp(node);
	}

	// Context menu: one menu for the whole tree, filled for the row it opened
	// on. Blank space gets the folder-level commands.
	let menuNode = $state<WorkspaceTreeNode | null>(null);

	// Drag and drop with pointer events, which work the same in every webview.
	let drag = $state<{
		path: string;
		name: string;
		startX: number;
		startY: number;
		x: number;
		y: number;
		active: boolean;
	} | null>(null);
	let dropFolder = $state<string | null>(null);
	let suppressClick = false;
	let expandTimer: ReturnType<typeof setTimeout> | undefined;

	function startDrag(event: PointerEvent) {
		if (event.button !== 0 || workspaceTree.renamingPath) return;
		const node = nodeAt(event);
		if (!node) return;
		drag = {
			path: node.relativePath,
			name: displayName(node),
			startX: event.clientX,
			startY: event.clientY,
			x: event.clientX,
			y: event.clientY,
			active: false,
		};
	}

	function dropFolderAt(x: number, y: number, path: string): string | null {
		const element = document.elementFromPoint(x, y);
		if (!element || !areaElement?.contains(element)) return null;
		const rowPath = element
			.closest('[data-tree-path]')
			?.getAttribute('data-tree-path');
		let folder = '';
		if (rowPath != null) {
			const node = findTreeNode(workspaceTree.tree, rowPath);
			folder =
				node?.kind === 'folder' ? rowPath : (parentPathOf(rowPath) ?? '');
		}
		return canMoveInto(path, folder) ? folder : null;
	}

	function moveDrag(event: PointerEvent) {
		if (!drag) return;
		if (
			!drag.active &&
			Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < 5
		)
			return;
		drag.active = true;
		drag.x = event.clientX;
		drag.y = event.clientY;
		const folder = dropFolderAt(event.clientX, event.clientY, drag.path);
		if (folder !== dropFolder) {
			clearTimeout(expandTimer);
			// Hovering a closed folder opens it, so deep folders are reachable.
			if (folder && !workspaceTree.isExpanded(folder))
				expandTimer = setTimeout(
					() => workspaceTree.setExpanded(folder, true),
					600,
				);
		}
		dropFolder = folder;
	}

	function endDrag() {
		const finished = drag;
		const target = dropFolder;
		clearTimeout(expandTimer);
		drag = null;
		dropFolder = null;
		if (!finished?.active) return;
		suppressClick = true;
		// A drop outside the tree fires no click to swallow.
		setTimeout(() => (suppressClick = false), 0);
		const node = findTreeNode(workspaceTree.tree, finished.path);
		if (node && target !== null) void moveTreeNode(node, target);
	}

	function cancelDrag() {
		clearTimeout(expandTimer);
		drag = null;
		dropFolder = null;
	}

	function menuFolder(): string {
		if (!menuNode) return '';
		return menuNode.kind === 'folder'
			? menuNode.relativePath
			: (parentPathOf(menuNode.relativePath) ?? '');
	}
</script>

<svelte:window
	onpointermove={moveDrag}
	onpointerup={endDrag}
	onpointercancel={cancelDrag}
	onblur={cancelDrag}
	onkeydown={(event) => {
		if (drag && event.key === 'Escape') cancelDrag();
	}}
/>

{#key workspace.state?.workspaceId}
	<WorkspaceTreeRefresh workspaceId={workspace.state?.workspaceId} />
{/key}

<Sidebar.SidebarGroup class="flex min-h-0 flex-1 flex-col">
	<div class="flex items-center gap-0.5 pr-1">
		<Sidebar.SidebarGroupLabel class="flex-1">Files</Sidebar.SidebarGroupLabel>
		{@render headerAction('New note', NotePencil, () => void createNote())}
		{@render headerAction(
			'New folder',
			FolderSimplePlus,
			() => void createFolder(),
		)}
		{@render headerAction('Collapse all', ArrowsInLineVertical, () =>
			workspaceTree.collapseAll(),
		)}
	</div>
	<Sidebar.SidebarGroupContent class="flex min-h-0 flex-1 flex-col">
		<ContextMenu.Root>
			<ContextMenu.Trigger>
				{#snippet child({ props })}
					<div
						{...props}
						bind:this={areaElement}
						class="flex min-h-24 flex-1 flex-col pb-6"
						oncontextmenucapture={(event) => {
							menuNode = nodeAt(event);
							if (menuNode) workspaceTree.selectedPath = menuNode.relativePath;
						}}
					>
						{#if workspaceTree.loading && workspaceTree.tree.length === 0}
							<div class="flex flex-col gap-1 px-2 py-1">
								{#each [0, 1, 2, 3, 4] as i (i)}
									<Skeleton class="h-7 w-full" />
								{/each}
							</div>
						{:else if workspaceTree.tree.length === 0}
							<p class="px-3 py-2 text-xs text-muted-foreground">
								This folder is empty.
							</p>
						{:else}
							<Sidebar.SidebarMenu
								bind:ref={treeElement}
								role="tree"
								aria-label="Files"
								class={dropFolder === ''
									? 'rounded-xl ring-2 ring-sidebar-ring'
									: ''}
								onkeydown={handleKeydown}
								onclick={handleClick}
								ondblclick={handleDoubleClick}
								onpointerdown={startDrag}
							>
								{#each rows as row, index (row.node.relativePath)}
									<TreeNode
										{row}
										expanded={workspaceTree.isExpanded(row.node.relativePath)}
										selected={index === focusIndex}
										active={row.node.relativePath === activePath}
										tabbable={index === tabbableIndex}
										renaming={workspaceTree.renamingPath ===
											row.node.relativePath}
										dropTarget={drag?.active === true &&
											dropFolder === row.node.relativePath}
										dragging={drag?.active === true &&
											drag.path === row.node.relativePath}
										onrename={(name) => void finishRename(row.node, name)}
										oncancelrename={() => cancelRename(row.node)}
									/>
								{/each}
							</Sidebar.SidebarMenu>
						{/if}
					</div>
				{/snippet}
			</ContextMenu.Trigger>
			<ContextMenu.Content
				class="w-56"
				onCloseAutoFocus={(event) => {
					event.preventDefault();
					if (!workspaceTree.renamingPath)
						focusPath(workspaceTree.selectedPath);
				}}
			>
				{#if menuNode?.kind === 'file'}
					{@const node = menuNode}
					<ContextMenu.Group>
						{#if opensHere(node)}
							<ContextMenu.Item onclick={() => void openTreeNode(node)}>
								Open
							</ContextMenu.Item>
							<ContextMenu.Item
								onclick={() => void openTreeNode(node, { keep: true })}
							>
								Open in new tab
							</ContextMenu.Item>
						{/if}
						{#if capabilities.openWithDefaultApp}
							<ContextMenu.Item onclick={() => void openWithDefaultApp(node)}>
								Open in default app
							</ContextMenu.Item>
						{/if}
					</ContextMenu.Group>
					<ContextMenu.Separator />
					<ContextMenu.Group>
						<ContextMenu.Item onclick={() => startRename(node)}>
							Rename…
							<ContextMenu.Shortcut>F2</ContextMenu.Shortcut>
						</ContextMenu.Item>
						<ContextMenu.Item onclick={() => void duplicateTreeNode(node)}>
							Duplicate
						</ContextMenu.Item>
						{#if capabilities.revealInFileManager}
							<ContextMenu.Item onclick={() => void revealTreeNode(node)}>
								{reveal}
							</ContextMenu.Item>
						{/if}
					</ContextMenu.Group>
					<ContextMenu.Separator />
					<ContextMenu.Group>
						<ContextMenu.Item
							variant="destructive"
							onclick={() => requestTrash(node)}
						>
							Move to Trash
							<ContextMenu.Shortcut>{trashShortcut}</ContextMenu.Shortcut>
						</ContextMenu.Item>
					</ContextMenu.Group>
				{:else}
					{@const node = menuNode}
					<ContextMenu.Group>
						<ContextMenu.Item onclick={() => void createNote(menuFolder())}>
							New note
						</ContextMenu.Item>
						<ContextMenu.Item onclick={() => void createFolder(menuFolder())}>
							New folder
						</ContextMenu.Item>
					</ContextMenu.Group>
					<ContextMenu.Separator />
					<ContextMenu.Group>
						{#if node}
							<ContextMenu.Item onclick={() => startRename(node)}>
								Rename…
								<ContextMenu.Shortcut>F2</ContextMenu.Shortcut>
							</ContextMenu.Item>
							{#if capabilities.revealInFileManager}
								<ContextMenu.Item onclick={() => void revealTreeNode(node)}>
									{reveal}
								</ContextMenu.Item>
							{/if}
						{:else}
							{#if capabilities.revealInFileManager}
								<ContextMenu.Item
									onclick={() =>
										void getNouraClient().workspaces.showInFolder('root')}
								>
									{reveal}
								</ContextMenu.Item>
							{/if}
							<ContextMenu.Item onclick={() => workspaceTree.collapseAll()}>
								Collapse all
							</ContextMenu.Item>
						{/if}
					</ContextMenu.Group>
					{#if node}
						<ContextMenu.Separator />
						<ContextMenu.Group>
							<ContextMenu.Item
								variant="destructive"
								onclick={() => requestTrash(node)}
							>
								Move to Trash
								<ContextMenu.Shortcut>{trashShortcut}</ContextMenu.Shortcut>
							</ContextMenu.Item>
						</ContextMenu.Group>
					{/if}
				{/if}
			</ContextMenu.Content>
		</ContextMenu.Root>
	</Sidebar.SidebarGroupContent>
</Sidebar.SidebarGroup>

{#if drag?.active}
	<div
		aria-hidden="true"
		class="pointer-events-none fixed z-50 max-w-48 truncate rounded-lg bg-popover px-2 py-1 text-xs text-popover-foreground shadow-md ring-1 ring-border"
		style="left: {drag.x + 12}px; top: {drag.y + 8}px"
	>
		{drag.name}
	</div>
{/if}

<AlertDialog.Root
	open={trashCandidate !== null}
	onOpenChange={(open) => {
		if (!open) trashCandidate = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>
				Move “{trashCandidate ? displayName(trashCandidate) : ''}” to the trash?
			</AlertDialog.Title>
			<AlertDialog.Description>
				{#if capabilities.systemTrash}
					The folder and everything in it go to the trash. You can restore them
					from there.
				{:else}
					The folder and everything in it move to .noura/trash in this
					workspace.
				{/if}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				variant="destructive"
				onclick={() => {
					const node = trashCandidate;
					trashCandidate = null;
					if (node) void trash(node);
				}}
			>
				Move to Trash
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

{#snippet headerAction(label: string, Icon: typeof NotePencil, run: () => void)}
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<Button
					{...props}
					variant="ghost"
					size="icon-xs"
					aria-label={label}
					onclick={run}
				>
					<Icon />
				</Button>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="bottom">{label}</Tooltip.Content>
	</Tooltip.Root>
{/snippet}
