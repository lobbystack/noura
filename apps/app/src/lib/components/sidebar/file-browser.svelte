<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { workspaceTree } from '$lib/workspace-tree.svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import TreeNode from '$lib/components/tree-node.svelte';

	onMount(() => {
		if (browser) void workspaceTree.start();
	});
</script>

<Sidebar.SidebarGroup class="h-full">
	<Sidebar.SidebarGroupLabel>Workspace files</Sidebar.SidebarGroupLabel>
	<Sidebar.SidebarGroupContent>
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
			<Sidebar.SidebarMenu>
				{#each workspaceTree.tree as node (node.relativePath)}
					<TreeNode {node} depth={0} />
				{/each}
			</Sidebar.SidebarMenu>
		{/if}
	</Sidebar.SidebarGroupContent>
</Sidebar.SidebarGroup>
