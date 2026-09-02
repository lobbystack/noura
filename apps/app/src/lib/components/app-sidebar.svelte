<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { getNouraClient } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import TasksViews from '$lib/components/sidebar/tasks-views.svelte';
	import FileBrowser from '$lib/components/sidebar/file-browser.svelte';

	let workspaceName = $state<string | null>(null);

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

	onMount(() => {
		if (!browser) return;
		void getNouraClient()
			.manifest.read()
			.then((manifest) => {
				workspaceName = manifest.name;
			})
			.catch(() => {
				workspaceName = null;
			});
	});
</script>

<Sidebar.Sidebar class="md:start-14" collapsible="offcanvas">
	<Sidebar.SidebarHeader class="px-3 py-2.5">
		<div class="flex items-center gap-2">
			<div
				class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-primary text-xs font-bold text-primary-foreground"
			>
				NR
			</div>
			<span class="min-w-0 truncate text-sm font-semibold">
				{workspaceName ?? 'Noura'}
			</span>
		</div>
	</Sidebar.SidebarHeader>

	<Sidebar.SidebarContent>
		{#if Section}
			<Section />
		{/if}
	</Sidebar.SidebarContent>
</Sidebar.Sidebar>
