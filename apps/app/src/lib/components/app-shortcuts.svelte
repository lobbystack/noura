<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { subscribeAppMenu, type AppMenuCommand } from '@noura/workspace';
	import {
		shortcutCommand,
		shortcutOnce,
		type ShortcutSource,
	} from '$lib/app-shortcuts';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { createFolder, createNote } from '$lib/file-actions';
	import { hostOs } from '$lib/host-os';
	import { getAppPlatform } from '$lib/platform';
	import { plugins } from '$lib/plugins.svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import {
		isSectionCollapsed,
		nextZoom,
		preferences,
		toggleSection,
	} from '$lib/preferences.svelte';
	import { routePlugin } from '$lib/plugin-routes';
	import { closeActiveTab, cycleTab } from '$lib/tab-actions';
	import { useSidebar } from '$lib/components/ui/sidebar/index.js';

	const settings = getSettingsDialog();
	const sidebar = useSidebar();
	const os = hostOs();

	const ROUTES: Partial<Record<AppMenuCommand, string>> = {
		'go-home': '/inbox',
		'go-files': '/files',
		'go-tasks': '/tasks',
		'go-calendar': '/calendar',
		'go-projects': '/projects',
		'go-ai': '/ai',
	};

	// On a Mac the menu bar and the key press can both report one shortcut;
	// run each press once.
	const once = shortcutOnce();

	async function run(command: AppMenuCommand, source: ShortcutSource) {
		if (!once(command, source, performance.now())) return;
		const path = ROUTES[command];
		if (path) {
			const pluginId = routePlugin(path);
			if (!pluginId || plugins.isEnabled(pluginId)) await goto(path);
			return;
		}
		switch (command) {
			case 'new-note':
				await createNote();
				break;
			case 'new-folder':
				// The new folder opens for renaming in the tree, so show the tree.
				if (!sidebar.open) sidebar.setOpen(true);
				if (isSectionCollapsed('files')) toggleSection('files');
				await createFolder();
				break;
			case 'quick-open':
				commandPalette.show('files');
				break;
			case 'search':
				commandPalette.show('search');
				break;
			case 'close-tab':
				await closeActiveTab();
				break;
			case 'settings':
				settings.show();
				break;
			case 'toggle-sidebar':
				sidebar.toggle();
				break;
			case 'zoom-in':
				preferences.update({ zoom: nextZoom(preferences.zoom, 1) });
				break;
			case 'zoom-out':
				preferences.update({ zoom: nextZoom(preferences.zoom, -1) });
				break;
			case 'zoom-reset':
				preferences.update({ zoom: 1 });
				break;
			case 'next-tab':
				await cycleTab(1);
				break;
			case 'previous-tab':
				await cycleTab(-1);
				break;
		}
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.defaultPrevented || event.repeat) return;
		const command = shortcutCommand(event, os);
		if (!command) return;
		// A browser zooms the page itself.
		if (command.startsWith('zoom-') && getAppPlatform() === 'web') return;
		event.preventDefault();
		void run(command, 'key');
	}

	onMount(() => {
		if (getAppPlatform() !== 'desktop') return;
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		void subscribeAppMenu((command) => void run(command, 'menu'))
			.then((stop) => {
				if (disposed) stop();
				else unsubscribe = stop;
			})
			.catch(() => {
				// Keyboard shortcuts still work without the menu bar.
			});
		return () => {
			disposed = true;
			unsubscribe?.();
		};
	});
</script>

<svelte:window onkeydown={handleKeydown} />
