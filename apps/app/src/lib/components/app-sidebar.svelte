<script lang="ts">
	import { page } from '$app/state';
	import { plugins } from '$lib/plugins.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import TasksViews from '$lib/components/sidebar/tasks-views.svelte';
	import FileBrowser from '$lib/components/sidebar/file-browser.svelte';

	const SECTION_COMPONENTS: Record<
		string,
		typeof TasksViews | typeof FileBrowser
	> = {
		'tasks-views': TasksViews,
		'file-browser': FileBrowser,
	};

	const activeModule = $derived(
		sidebarModuleFor(page.url.pathname, new Set(plugins.enabledIds)),
	);
	const Section = $derived(
		activeModule ? SECTION_COMPONENTS[activeModule.id] : undefined,
	);
</script>

<Sidebar.Sidebar
	class="md:start-14"
	style="top: 3rem; height: calc(100svh - 3rem);"
	collapsible="offcanvas"
>
	<Sidebar.SidebarContent>
		{#if Section}
			<Section />
		{/if}
	</Sidebar.SidebarContent>
</Sidebar.Sidebar>
