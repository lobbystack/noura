<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { tasksData } from '$lib/tasks-data.svelte';
	import type { TaskView, TaskViewId } from '$lib/tasks/filters';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';

	const VIEW_MODES: readonly TaskViewId[] = [
		'today',
		'upcoming',
		'all',
		'completed',
	];

	const currentFolder = $derived(
		page.url.searchParams.get('view') === 'folder'
			? (page.url.searchParams.get('folder') ?? '')
			: null,
	);

	const currentMode = $derived.by((): TaskViewId => {
		if (currentFolder !== null) return 'folder';
		const mode = page.url.searchParams.get('view') ?? 'today';
		return VIEW_MODES.includes(mode as TaskViewId)
			? (mode as TaskViewId)
			: 'today';
	});

	const folderGroups = $derived.by(() => {
		const groups: Record<string, number> = {};
		for (const task of tasksData.tasks) {
			const parts = task.relativePath.split('/');
			const folder = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
			groups[folder] = (groups[folder] ?? 0) + 1;
		}
		return Object.entries(groups).sort(([left], [right]) =>
			left.localeCompare(right),
		);
	});

	function viewHref(mode: TaskViewId): string {
		return withSelected(`view=${mode}`);
	}

	function folderHref(folder: string): string {
		const params = new URLSearchParams({ view: 'folder', folder });
		const selected = page.url.searchParams.get('selected');
		if (selected) params.set('selected', selected);
		return `/tasks?${params.toString()}`;
	}

	function withSelected(query: string): string {
		const selected = page.url.searchParams.get('selected');
		return selected
			? `/tasks?${query}&selected=${encodeURIComponent(selected)}`
			: `/tasks?${query}`;
	}
</script>

<Sidebar.SidebarGroup class="h-full">
	<Sidebar.SidebarGroupLabel>Views</Sidebar.SidebarGroupLabel>
	<Sidebar.SidebarGroupContent>
		<Sidebar.SidebarMenu>
			{#each VIEW_MODES as mode (mode)}
				<Sidebar.SidebarMenuItem>
					<Sidebar.SidebarMenuButton
						isActive={currentMode === mode}
						size="sm"
						onclick={() => void goto(viewHref(mode))}
					>
						{mode.charAt(0).toUpperCase() + mode.slice(1)}
					</Sidebar.SidebarMenuButton>
				</Sidebar.SidebarMenuItem>
			{/each}
		</Sidebar.SidebarMenu>

		<div
			class="mt-4 px-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
		>
			Projects
		</div>
		<Sidebar.SidebarMenu>
			{#each tasksData.projects as project (project.id)}
				<Sidebar.SidebarMenuItem>
					<Sidebar.SidebarMenuButton
						size="sm"
						class="text-muted-foreground"
						aria-disabled="true"
						title="Project filtering arrives with the tasks module's project view"
					>
						<span class="truncate">{project.title}</span>
					</Sidebar.SidebarMenuButton>
				</Sidebar.SidebarMenuItem>
			{:else}
				<p class="px-2 py-1.5 text-xs text-muted-foreground">No projects</p>
			{/each}
		</Sidebar.SidebarMenu>

		{#if currentFolder !== null || folderGroups.length > 0}
			<div
				class="mt-4 px-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
			>
				Folders
			</div>
			<Sidebar.SidebarMenu>
				{#each folderGroups as [folder, count] (folder)}
					<Sidebar.SidebarMenuItem>
						<Sidebar.SidebarMenuButton
							isActive={currentFolder === folder}
							size="sm"
							onclick={() => void goto(folderHref(folder))}
						>
							<span class="truncate">{folder || '/'}</span>
							<span class="ml-auto text-xs text-muted-foreground">{count}</span>
						</Sidebar.SidebarMenuButton>
					</Sidebar.SidebarMenuItem>
				{/each}
			</Sidebar.SidebarMenu>
		{/if}
	</Sidebar.SidebarGroupContent>
</Sidebar.SidebarGroup>
