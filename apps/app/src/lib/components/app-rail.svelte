<script lang="ts">
	import { page } from '$app/stores';
	import { getSettingsDialog } from '$lib/settings.svelte';
	const settings = getSettingsDialog();
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
	import type { NavigationId } from '$lib/plugin-order';
	import { commandPalette } from '$lib/command-palette.svelte';
	import type { Component } from 'svelte';

	type PluginRailEntry = {
		label: string;
		path: string;
		icon: Component;
	};

	const pluginEntries: Partial<Record<NavigationId, PluginRailEntry>> = {
		inbox: { label: 'Inbox', path: '/inbox', icon: Tray },
		ai: { label: 'AI', path: '/ai', icon: Sparkle },
		notes: { label: 'Notes', path: '/notes', icon: NotePencil },
		tasks: { label: 'Tasks', path: '/tasks', icon: Checks },
		calendar: { label: 'Calendar', path: '/calendar', icon: Calendar },
		projects: { label: 'Projects', path: '/projects', icon: FolderOpen },
	};

	let draggedNavigationId = $state<NavigationId | null>(null);
	let dropTargetId = $state<NavigationId | null>(null);
	let dropAfter = $state(false);
	let pointer: {
		id: number;
		pluginId: NavigationId;
		x: number;
		y: number;
	} | null = null;
	let suppressClick = false;

	function startPointer(event: PointerEvent, pluginId: NavigationId) {
		if (event.button !== 0) return;
		suppressClick = false;
		pointer = {
			id: event.pointerId,
			pluginId,
			x: event.clientX,
			y: event.clientY,
		};
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function movePointer(event: PointerEvent) {
		if (!pointer || pointer.id !== event.pointerId) return;
		if (
			!draggedNavigationId &&
			Math.hypot(event.clientX - pointer.x, event.clientY - pointer.y) < 5
		)
			return;
		draggedNavigationId = pointer.pluginId;
		suppressClick = true;
		const target = document
			.elementFromPoint(event.clientX, event.clientY)
			?.closest<HTMLElement>('[data-plugin-id]');
		const id = target?.dataset.pluginId as NavigationId | undefined;
		dropTargetId = id && id !== draggedNavigationId ? id : null;
		if (target) {
			const rect = target.getBoundingClientRect();
			dropAfter = event.clientY > rect.top + rect.height / 2;
		}
	}

	function clearDrag() {
		pointer = null;
		draggedNavigationId = null;
		dropTargetId = null;
	}

	function finishPointer(event: PointerEvent) {
		if (!pointer || pointer.id !== event.pointerId) return;
		movePointer(event);
		if (draggedNavigationId && dropTargetId)
			plugins.move(draggedNavigationId, dropTargetId, dropAfter);
		clearDrag();
	}
</script>

<svelte:window
	onpointermove={movePointer}
	onpointerup={finishPointer}
	onpointercancel={clearDrag}
	onblur={clearDrag}
	onkeydown={(event) => {
		if (event.key === 'Escape') clearDrag();
	}}
/>

<nav
	class="flex w-14 shrink-0 flex-col justify-between border-r border-sidebar-border bg-sidebar py-3"
	aria-label="Primary navigation"
>
	<div class="flex flex-col items-center gap-1">
		{#each plugins.orderedSidebarPluginIds as pluginId (pluginId)}
			{@const entry = pluginEntries[pluginId]}
			{#if entry && (pluginId === 'inbox' || plugins.isEnabled(pluginId))}
				{@const active = $page.url.pathname.startsWith(entry.path)}
				<a
					href={entry.path}
					draggable="false"
					data-plugin-id={pluginId}
					class={cn(
						'relative flex size-9 touch-none select-none items-center justify-center rounded-xl transition-colors',
						active
							? 'bg-sidebar-accent text-sidebar-accent-foreground'
							: 'text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
						draggedNavigationId === pluginId && 'opacity-40',
						draggedNavigationId && 'cursor-grabbing',
					)}
					aria-label={entry.label}
					title={entry.label}
					onpointerdown={(event) => startPointer(event, pluginId)}
					ondragstart={(event) => event.preventDefault()}
					onclick={(event) => {
						if (suppressClick) {
							event.preventDefault();
							suppressClick = false;
						}
					}}
				>
					{#if dropTargetId === pluginId}<span
							aria-hidden="true"
							class={cn(
								'pointer-events-none absolute inset-x-0 h-0.5 bg-primary',
								dropAfter ? '-bottom-0.5' : '-top-0.5',
							)}
						></span>{/if}
					<entry.icon class="size-5" weight={active ? 'fill' : 'regular'} />
				</a>
			{/if}
		{/each}
	</div>
	<div class="flex flex-col items-center gap-1">
		<button
			type="button"
			class={cn(
				'flex size-9 items-center justify-center rounded-xl transition-colors',
				'text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
			)}
			aria-label="Search (Cmd+K)"
			title="Search (Cmd+K)"
			onclick={() => commandPalette.show()}
		>
			<MagnifyingGlass class="size-5" weight="regular" />
		</button>
		<button
			id="open-settings"
			type="button"
			class={cn(
				'flex size-9 items-center justify-center rounded-xl transition-colors',
				'text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
			)}
			aria-label="Settings"
			title="Settings (⌘,)"
			aria-haspopup="dialog"
			onclick={() => settings.show()}><GearSix class="size-5" /></button
		>
	</div>
</nav>
