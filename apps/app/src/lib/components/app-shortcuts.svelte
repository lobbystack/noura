<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { subscribeAppMenu, type AppMenuCommand } from '@noura/workspace';
	import { shortcutCommand } from '$lib/app-shortcuts';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { createFolder, createNote } from '$lib/file-actions';
	import { hostOs } from '$lib/host-os';
	import { getAppPlatform } from '$lib/platform';
	import { plugins } from '$lib/plugins.svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { routePlugin } from '$lib/plugin-routes';
	import { showsFileTree } from '$lib/sidebar-modules';
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
	// run each command once.
	const lastRun: Partial<Record<AppMenuCommand, number>> = {};

	async function run(command: AppMenuCommand) {
		const now = performance.now();
		if (now - (lastRun[command] ?? -Infinity) < 300) return;
		lastRun[command] = now;
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
				if (!showsFileTree(page.url.pathname)) await goto('/files');
				if (!sidebar.open) sidebar.setOpen(true);
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
		event.preventDefault();
		void run(command);
	}

	onMount(() => {
		if (getAppPlatform() !== 'desktop') return;
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		void subscribeAppMenu((command) => void run(command))
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
