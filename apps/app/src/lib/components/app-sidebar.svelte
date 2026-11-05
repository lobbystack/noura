<script lang="ts">
	import { page } from '$app/state';
	import { plugins } from '$lib/plugins.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { getRouteSidebar } from '$lib/route-sidebar.svelte';
	import TasksViews from '$lib/components/sidebar/tasks-views.svelte';
	import FileBrowser from '$lib/components/sidebar/file-browser.svelte';
	import WorkspaceSwitcher from '$lib/components/workspace-switcher.svelte';
	import SidebarResizeHandle from '$lib/components/sidebar/sidebar-resize-handle.svelte';

	let { open = true }: { open?: boolean } = $props();

	const routeSidebar = getRouteSidebar();

	// The sidebar looks the same on every page: the page's own section, when
	// its plugin adds one, then the file tree.
	const activeModule = $derived(
		sidebarModuleFor(page.url.pathname, new Set(plugins.activeIds)),
	);
</script>

<!-- Closing narrows the sidebar toward the rail instead of sliding it
across the rail; its content keeps its width and is clipped. -->
<Sidebar.Sidebar
	class="overflow-hidden md:start-14 group-data-[collapsible=offcanvas]:start-14 group-data-[collapsible=offcanvas]:w-0 group-data-[collapsible=offcanvas]:group-data-[side=left]:border-e-0 [&>[data-slot=sidebar-inner]]:w-(--sidebar-width) [&>[data-slot=sidebar-inner]]:shrink-0"
	collapsible="offcanvas"
	inert={!open}
	aria-hidden={!open}
>
	<Sidebar.SidebarHeader
		class="h-(--app-titlebar-height) shrink-0 justify-center border-b border-sidebar-border pr-2 pl-6 py-0"
		data-tauri-drag-region
	>
		<WorkspaceSwitcher />
	</Sidebar.SidebarHeader>
	<Sidebar.SidebarContent>
		{#if activeModule?.id === 'tasks-views'}
			<TasksViews />
			<Sidebar.SidebarSeparator />
		{:else if activeModule?.id === 'projects' && routeSidebar.content}
			{@render routeSidebar.content()}
			<Sidebar.SidebarSeparator />
		{/if}
		<FileBrowser />
	</Sidebar.SidebarContent>
	<SidebarResizeHandle />
</Sidebar.Sidebar>
