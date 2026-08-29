<script lang="ts">
	import { page } from '$app/stores';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import House from 'phosphor-svelte/lib/House';
	import Tray from 'phosphor-svelte/lib/Tray';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Calendar from 'phosphor-svelte/lib/Calendar';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import MagnifyingGlass from 'phosphor-svelte/lib/MagnifyingGlass';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import GearSix from 'phosphor-svelte/lib/GearSix';
	import { cn } from '$lib/utils.js';

	const route = $derived($page.url.pathname.split('/')[1] ?? 'home');

	const sections = {
		tasks: {
			title: 'Views',
			items: [
				{ label: 'Today', icon: Checks, active: false },
				{ label: 'Upcoming', icon: Calendar, active: false },
				{ label: 'All Tasks', icon: Checks, active: true },
			],
		},
		notes: {
			title: 'Recent',
			items: [{ label: 'Getting started', icon: NotePencil, active: true }],
		},
		projects: {
			title: 'Projects',
			items: [{ label: 'All projects', icon: FolderOpen, active: true }],
		},
	} as const;
</script>

<Sidebar.Sidebar class="md:start-14" collapsible="offcanvas">
	<Sidebar.SidebarHeader class="px-3 py-2.5">
		<div class="flex items-center gap-2">
			<div
				class="flex size-7 items-center justify-center rounded-lg bg-primary text-xs font-bold text-primary-foreground"
			>
				NR
			</div>
			<span class="truncate text-sm font-semibold">Noura</span>
		</div>
	</Sidebar.SidebarHeader>

	<Sidebar.SidebarContent>
		{#if route === 'tasks' || route === 'notes' || route === 'projects'}
			{@const section = sections[route as keyof typeof sections]}
			<Sidebar.SidebarGroup>
				<Sidebar.SidebarGroupLabel>{section.title}</Sidebar.SidebarGroupLabel>
				<Sidebar.SidebarGroupContent>
					<Sidebar.SidebarMenu>
						{#each section.items as item (item.label)}
							<Sidebar.SidebarMenuItem>
								<Sidebar.SidebarMenuButton isActive={item.active}>
									<item.icon />
									<span>{item.label}</span>
								</Sidebar.SidebarMenuButton>
							</Sidebar.SidebarMenuItem>
						{/each}
					</Sidebar.SidebarMenu>
				</Sidebar.SidebarGroupContent>
			</Sidebar.SidebarGroup>
		{:else}
			<Sidebar.SidebarGroup>
				<Sidebar.SidebarGroupContent>
					<div class="px-3 py-8 text-center text-xs text-muted-foreground">
						Select a section to browse
					</div>
				</Sidebar.SidebarGroupContent>
			</Sidebar.SidebarGroup>
		{/if}
	</Sidebar.SidebarContent>
</Sidebar.Sidebar>
