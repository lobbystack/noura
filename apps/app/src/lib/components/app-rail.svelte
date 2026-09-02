<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
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
	import { plugins } from '$lib/plugins.svelte';
	import type { Component } from 'svelte';

	type RailEntry = readonly [string, string, Component, string?];

	const primary: readonly RailEntry[] = [
		['Home', '/home', House],
		['Inbox', '/inbox', Tray],
		['Notes', '/notes', NotePencil, 'notes'],
		['Tasks', '/tasks', Checks, 'tasks'],
		['Calendar', '/calendar', Calendar, 'calendar'],
		['Projects', '/projects', FolderOpen, 'projects'],
	];

	const secondary: readonly RailEntry[] = [
		['Search', '/search', MagnifyingGlass],
		['AI', '/ai', Sparkle],
		['Settings', '/settings', GearSix],
	];
</script>

<nav
	class="flex w-14 shrink-0 flex-col justify-between border-r border-sidebar-border bg-sidebar py-3"
	aria-label="Primary navigation"
>
	<div class="flex flex-col items-center gap-1">
		{#each primary as [label, path, Icon, pluginId] (path)}
			{#if !pluginId || plugins.isEnabled(pluginId)}
				{@render railEntry(label, path, Icon)}
			{/if}
		{/each}
	</div>
	<div class="flex flex-col items-center gap-1">
		{#each secondary as [label, path, Icon] (path)}
			{@render railEntry(label, path, Icon)}
		{/each}
	</div>
</nav>

{#snippet railEntry(label: string, path: string, Icon: Component)}
	{@const active = $page.url.pathname.startsWith(path)}
	<a
		href={path}
		class={cn(
			'flex size-9 items-center justify-center rounded-xl transition-colors',
			active
				? 'bg-sidebar-accent text-sidebar-accent-foreground'
				: 'text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
		)}
		aria-label={label}
		title={label}
	>
		<Icon class="size-5" weight={active ? 'fill' : 'regular'} />
	</a>
{/snippet}
