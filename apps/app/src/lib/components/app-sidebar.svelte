<script lang="ts">
	import { page } from '$app/state';
	import { plugins } from '$lib/plugins.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { getRouteSidebar } from '$lib/route-sidebar.svelte';
	const routeSidebar = getRouteSidebar();
	import TasksViews from '$lib/components/sidebar/tasks-views.svelte';
	import FileBrowser from '$lib/components/sidebar/file-browser.svelte';
	import WorkspaceSwitcher from '$lib/components/workspace-switcher.svelte';

	const SECTION_COMPONENTS: Record<
		string,
		typeof TasksViews | typeof FileBrowser
	> = {
		'tasks-views': TasksViews,
		'file-browser': FileBrowser,
	};

	const activeModule = $derived(
		sidebarModuleFor(page.url.pathname, new Set(plugins.activeIds)),
	);
	const Section = $derived(
		activeModule ? SECTION_COMPONENTS[activeModule.id] : undefined,
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
		{#if Section}<Section
			/>{:else if activeModule?.id === 'projects'}{@render routeSidebar.content?.()}{/if}
	</Sidebar.SidebarContent>
</Sidebar.Sidebar>
