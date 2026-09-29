<script lang="ts">
	import { page } from '$app/state';
	import { plugins } from '$lib/plugins.svelte';
	import { showsFileTree, sidebarModuleFor } from '$lib/sidebar-modules';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { getRouteSidebar } from '$lib/route-sidebar.svelte';
	import TasksViews from '$lib/components/sidebar/tasks-views.svelte';
	import FileBrowser from '$lib/components/sidebar/file-browser.svelte';
	import WorkspaceSwitcher from '$lib/components/workspace-switcher.svelte';

	const routeSidebar = getRouteSidebar();

	const activeModule = $derived(
		sidebarModuleFor(page.url.pathname, new Set(plugins.activeIds)),
	);
	// Task and project routes keep their own section and show the file tree
	// under it, so opening one of their files never hides the tree.
	const withFiles = $derived(
		activeModule?.id !== 'file-browser' && showsFileTree(page.url.pathname),
	);
</script>

<Sidebar.Sidebar class="md:start-14" collapsible="offcanvas">
	<Sidebar.SidebarHeader
		class="h-(--app-titlebar-height) shrink-0 justify-center border-b border-sidebar-border pr-2 pl-6 py-0"
		data-tauri-drag-region
	>
		<WorkspaceSwitcher />
	</Sidebar.SidebarHeader>
	<Sidebar.SidebarContent>
		{#if activeModule?.id === 'file-browser'}
			<FileBrowser />
		{:else}
			{#if activeModule?.id === 'tasks-views'}
				<TasksViews />
			{:else if activeModule?.id === 'projects'}
				{@render routeSidebar.content?.()}
			{/if}
			{#if withFiles}
				<Sidebar.SidebarSeparator />
				<FileBrowser />
			{/if}
		{/if}
	</Sidebar.SidebarContent>
</Sidebar.Sidebar>
