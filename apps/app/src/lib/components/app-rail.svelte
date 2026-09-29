<script lang="ts">
	import { page } from '$app/state';
	import { tick, type Component } from 'svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import House from 'phosphor-svelte/lib/House';
	import Files from 'phosphor-svelte/lib/Files';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Calendar from 'phosphor-svelte/lib/Calendar';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import MagnifyingGlass from 'phosphor-svelte/lib/MagnifyingGlass';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import GearSix from 'phosphor-svelte/lib/GearSix';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import AppShortcuts from '$lib/components/app-shortcuts.svelte';
	import { cn } from '$lib/utils.js';
	import { plugins } from '$lib/plugins.svelte';
	import type { NavigationId } from '$lib/plugin-order';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { shortcutLabel } from '$lib/host-os';

	const settings = getSettingsDialog();

	type RailEntry = {
		label: string;
		path: string;
		icon: Component<{ class?: string; weight?: 'fill' | 'regular' }>;
	};

	// `notes` keeps its id so saved rail orders still apply; the section is Files.
	const entries: Partial<Record<NavigationId, RailEntry>> = {
		inbox: { label: 'Home', path: '/inbox', icon: House },
		ai: { label: 'AI', path: '/ai', icon: Sparkle },
		notes: { label: 'Files', path: '/files', icon: Files },
		tasks: { label: 'Tasks', path: '/tasks', icon: Checks },
		calendar: { label: 'Calendar', path: '/calendar', icon: Calendar },
		projects: { label: 'Projects', path: '/projects', icon: FolderOpen },
	};

	const visibleIds = $derived(
		plugins.orderedSidebarPluginIds.filter(
			(id) =>
				entries[id] &&
				(id === 'inbox' || id === 'notes' || plugins.isEnabled(id)),
		),
	);

	function isCurrent(path: string) {
		const current = page.url.pathname;
		// PDFs open from the file tree, so they belong to Files.
		if (path === '/files' && current.startsWith('/pdf')) return true;
		return current === path || current.startsWith(`${path}/`);
	}

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

	// Alt+Up and Alt+Down reorder the rail from the keyboard.
	async function moveWithKeyboard(event: KeyboardEvent, id: NavigationId) {
		if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown'))
			return;
		event.preventDefault();
		const index = visibleIds.indexOf(id);
		const down = event.key === 'ArrowDown';
		const neighbor = visibleIds[index + (down ? 1 : -1)];
		if (!neighbor) return;
		plugins.move(id, neighbor, down);
		await tick();
		document.querySelector<HTMLElement>(`[data-plugin-id="${id}"]`)?.focus();
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

<AppShortcuts />

<nav
	class="flex w-14 shrink-0 flex-col justify-between border-r border-sidebar-border bg-sidebar py-3"
	aria-label="Sections"
>
	<div class="flex flex-col items-center gap-1">
		{#each visibleIds as pluginId (pluginId)}
			{@const entry = entries[pluginId]!}
			{@const current = isCurrent(entry.path)}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<a
							{...props}
							href={entry.path}
							draggable="false"
							data-plugin-id={pluginId}
							aria-current={current ? 'page' : undefined}
							aria-label={entry.label}
							aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
							class={cn(
								'relative flex size-9 touch-none items-center justify-center rounded-xl transition-colors select-none outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring',
								current
									? 'bg-sidebar-accent text-sidebar-accent-foreground'
									: 'text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
								draggedNavigationId === pluginId && 'opacity-40',
								draggedNavigationId && 'cursor-grabbing',
							)}
							onpointerdown={(event) => startPointer(event, pluginId)}
							ondragstart={(event) => event.preventDefault()}
							onkeydown={(event) => void moveWithKeyboard(event, pluginId)}
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
							<entry.icon
								class="size-5"
								weight={current ? 'fill' : 'regular'}
							/>
						</a>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content side="right">{entry.label}</Tooltip.Content>
			</Tooltip.Root>
		{/each}
	</div>
	<div class="flex flex-col items-center gap-1">
		{@render railButton(
			`Search (${shortcutLabel(['Mod', 'K'])})`,
			MagnifyingGlass,
			() => commandPalette.show(),
		)}
		{@render railButton(
			`Settings (${shortcutLabel(['Mod', ','])})`,
			GearSix,
			() => settings.show(),
			'open-settings',
		)}
	</div>
</nav>

{#snippet railButton(
	label: string,
	Icon: typeof GearSix,
	run: () => void,
	id?: string,
)}
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<button
					{...props}
					{id}
					type="button"
					class="flex size-9 items-center justify-center rounded-xl text-sidebar-foreground/70 transition-colors outline-none hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring"
					aria-label={label.replace(/ \(.*\)$/, '')}
					aria-haspopup="dialog"
					onclick={run}
				>
					<Icon class="size-5" />
				</button>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="right">{label}</Tooltip.Content>
	</Tooltip.Root>
{/snippet}
