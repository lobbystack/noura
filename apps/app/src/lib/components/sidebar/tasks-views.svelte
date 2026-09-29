<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { tasksData } from '$lib/tasks-data.svelte';
	import type { TaskViewId } from '$lib/tasks/filters';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';

	const VIEW_MODES: readonly TaskViewId[] = [
		'today',
		'upcoming',
		'all',
		'completed',
	];

	const params = $derived(page.url.searchParams);
	const currentFolder = $derived(
		params.get('view') === 'folder' ? (params.get('folder') ?? '') : null,
	);
	const currentProject = $derived(
		params.get('view') === 'project' ? (params.get('project') ?? '') : null,
	);

	const currentMode = $derived.by((): TaskViewId => {
		if (currentFolder !== null) return 'folder';
		if (currentProject !== null) return 'project';
		const mode = params.get('view') ?? 'today';
		return VIEW_MODES.includes(mode as TaskViewId)
			? (mode as TaskViewId)
			: 'today';
	});

	const collator = new Intl.Collator(undefined, { sensitivity: 'base' });
	const folderGroups = $derived.by(() => {
		const groups: Record<string, number> = {};
		for (const task of tasksData.tasks) {
			const parts = task.relativePath.split('/');
			const folder = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
			groups[folder] = (groups[folder] ?? 0) + 1;
		}
		return Object.entries(groups).sort(([left], [right]) =>
			collator.compare(left, right),
		);
	});

	/** A tasks URL for a view, keeping the open task selected. */
	function href(query: Record<string, string>): string {
		const next = new URLSearchParams(query);
		const selected = params.get('selected');
		if (selected) next.set('selected', selected);
		return `/tasks?${next.toString()}`;
	}

	function label(mode: string): string {
		return mode.charAt(0).toUpperCase() + mode.slice(1);
	}
</script>

<Sidebar.SidebarGroup>
	<Sidebar.SidebarGroupLabel>Views</Sidebar.SidebarGroupLabel>
	<Sidebar.SidebarGroupContent>
		<Sidebar.SidebarMenu>
			{#each VIEW_MODES as mode (mode)}
				<Sidebar.SidebarMenuItem>
					<Sidebar.SidebarMenuButton
						isActive={currentMode === mode}
						size="sm"
						onclick={() => void goto(href({ view: mode }))}
					>
						{label(mode)}
					</Sidebar.SidebarMenuButton>
				</Sidebar.SidebarMenuItem>
			{/each}
		</Sidebar.SidebarMenu>
	</Sidebar.SidebarGroupContent>
</Sidebar.SidebarGroup>

{#if tasksData.projects.length > 0}
	<Sidebar.SidebarGroup>
		<Sidebar.SidebarGroupLabel>Projects</Sidebar.SidebarGroupLabel>
		<Sidebar.SidebarGroupContent>
			<Sidebar.SidebarMenu>
				{#each tasksData.projects as project (project.id)}
					<Sidebar.SidebarMenuItem>
						<Sidebar.SidebarMenuButton
							isActive={currentProject === project.id}
							size="sm"
							onclick={() =>
								void goto(href({ view: 'project', project: project.id }))}
						>
							<span class="truncate">{project.title}</span>
						</Sidebar.SidebarMenuButton>
					</Sidebar.SidebarMenuItem>
				{/each}
			</Sidebar.SidebarMenu>
		</Sidebar.SidebarGroupContent>
	</Sidebar.SidebarGroup>
{/if}

{#if currentFolder !== null || folderGroups.length > 0}
	<Sidebar.SidebarGroup>
		<Sidebar.SidebarGroupLabel>Folders</Sidebar.SidebarGroupLabel>
		<Sidebar.SidebarGroupContent>
			<Sidebar.SidebarMenu>
				{#each folderGroups as [folder, count] (folder)}
					<Sidebar.SidebarMenuItem>
						<Sidebar.SidebarMenuButton
							isActive={currentFolder === folder}
							size="sm"
							onclick={() => void goto(href({ view: 'folder', folder }))}
						>
							<span class="truncate">{folder || 'Top level'}</span>
						</Sidebar.SidebarMenuButton>
						<Sidebar.SidebarMenuBadge>{count}</Sidebar.SidebarMenuBadge>
					</Sidebar.SidebarMenuItem>
				{/each}
			</Sidebar.SidebarMenu>
		</Sidebar.SidebarGroupContent>
	</Sidebar.SidebarGroup>
{/if}
