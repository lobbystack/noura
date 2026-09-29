<script lang="ts">
	import { onMount, type Component, type Snippet } from 'svelte';
	import { ModeWatcher } from 'mode-watcher';
	import { beforeNavigate, goto, preloadCode } from '$app/navigation';
	import { page } from '$app/state';
	import { saveBeforeLeaving } from '$lib/editor/unsaved-changes';
	import {
		createTauriHostLifecycle,
		installPendingDraftCloseGuard,
	} from '@noura/workspace';
	import { SettingsDialog, setSettingsDialog } from '$lib/settings.svelte';
	import { RouteSidebar, setRouteSidebar } from '$lib/route-sidebar.svelte';
	import { commandPalette } from '$lib/command-palette.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { workspace, getNouraClient } from '$lib/state.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import { plugins, PLUGIN_ROUTES } from '$lib/plugins.svelte';
	import { FILES_ROUTE } from '$lib/navigation-targets';
	import { whenIdle } from '$lib/idle';
	import {
		flushPendingDrafts,
		hasPendingDrafts,
	} from '$lib/editor/pending-drafts.svelte';
	import { AppUpdates, setAppUpdates } from '$lib/app-updates.svelte';
	import { preferences } from '$lib/preferences.svelte';
	import TabsBar from '$lib/components/tabs-bar.svelte';
	import AppRail from '$lib/components/app-rail.svelte';
	import AppSidebar from '$lib/components/app-sidebar.svelte';
	import WorkspaceSwitcher from '$lib/components/workspace-switcher.svelte';
	import WorkspaceOnboarding from '$lib/components/workspace-onboarding.svelte';
	import WorkspaceLoading from '$lib/components/workspace-loading.svelte';
	import AppUpdateNotice from '$lib/components/app-update-notice.svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { Toaster } from '$lib/components/ui/sonner/index.js';

	let { children } = $props<{ children: Snippet }>();

	const settingsDialog = new SettingsDialog();
	setSettingsDialog(settingsDialog);
	setRouteSidebar(new RouteSidebar());
	setAppUpdates(new AppUpdates());

	/** How often ephemeral chats are checked for expiry while the app runs. */
	const CHAT_EXPIRY_INTERVAL_MS = 60 * 60 * 1000;

	// The settings dialog and the command palette load on first use. Neither
	// is needed to show the workspace, and together they pull in the sync,
	// AI and plugin settings code.
	let SettingsModal = $state<Component | null>(null);
	let Palette = $state<Component | null>(null);

	function loadSettings() {
		if (SettingsModal) return;
		void import('$lib/components/settings/settings-dialog.svelte').then(
			(module) => (SettingsModal = module.default),
		);
	}

	function loadPalette() {
		if (Palette) return;
		void import('$lib/components/command-palette.svelte').then(
			(module) => (Palette = module.default),
		);
	}

	$effect(() => {
		if (settingsDialog.open) loadSettings();
	});
	$effect(() => {
		if (commandPalette.open) loadPalette();
	});

	// Until a lazy component is mounted it cannot hear its own shortcut.
	// Once it is, it owns the key and this handler stays out of the way.
	function handleKeydown(event: KeyboardEvent) {
		if (!(event.metaKey || event.ctrlKey) || event.repeat) return;
		const key = event.key.toLowerCase();
		if (key === 'k' && !Palette) {
			event.preventDefault();
			commandPalette.show();
		} else if (key === ',' && !SettingsModal) {
			event.preventDefault();
			settingsDialog.show();
		}
	}

	// Effects below key on these values, not on `workspace.state`, which is
	// replaced on every read. Each runs once per workspace transition.
	const workspaceId = $derived(workspace.state?.workspaceId);
	const settledKey = $derived(workspace.settledKey);
	const readyWorkspaceId = $derived(
		workspace.isReady ? (workspace.state?.workspaceId ?? null) : null,
	);

	$effect(() => {
		tabsStore.setWorkspace(workspaceId);
	});

	// The engine broadcasts workspace:ready before the host event bridge
	// subscribes, so create/open flows would miss it. Reconcile plugins each
	// time the workspace settles (ready, idle, failed) so toggles and
	// navigation always match .noura/workspace.yaml.
	$effect(() => {
		if (settledKey) void plugins.sync();
	});

	// Ephemeral chats expire in the background, once per workspace and then
	// hourly. It is housekeeping: nothing waits for it.
	$effect(() => {
		const id = readyWorkspaceId;
		if (!id) return;
		const expire = () =>
			whenIdle(() => {
				if (workspace.state?.workspaceId !== id) return;
				void getNouraClient()
					.chats.expire(new Date().toISOString())
					.catch(() => {});
			}, 10_000);
		let cancel = expire();
		const interval = setInterval(() => {
			cancel();
			cancel = expire();
		}, CHAT_EXPIRY_INTERVAL_MS);
		return () => {
			cancel();
			clearInterval(interval);
		};
	});

	// Once the workspace shows, fetch the editor while the app is idle so the
	// first file opens without waiting for it.
	$effect(() => {
		if (!readyWorkspaceId) return;
		return whenIdle(() => {
			void preloadCode(FILES_ROUTE).catch(() => {});
			void import('@noura/editor').catch(() => {});
		}, 5_000);
	});

	// Turned-off modules genuinely simplify the workspace: routes backed by a
	// disabled plugin fall back to Home instead of rendering a dead surface.
	$effect(() => {
		if (!plugins.synced) return;
		const path = page.url.pathname;
		for (const [pluginId, route] of PLUGIN_ROUTES) {
			if (path.startsWith(route) && !plugins.isEnabled(pluginId)) {
				void goto('/inbox', { replaceState: true });
				return;
			}
		}
	});

	let allowedNavigation: string | null = null;
	beforeNavigate((navigation) => {
		const destination = navigation.to?.url;
		if (destination?.href === allowedNavigation) {
			allowedNavigation = null;
			return;
		}
		if (!hasPendingDrafts()) return;
		navigation.cancel();
		const proceed = () => {
			if (!destination) return;
			allowedNavigation = destination.href;
			void goto(destination, {
				replaceState: navigation.type === 'popstate',
			});
		};
		void saveBeforeLeaving(proceed).then((saved) => saved && proceed());
	});

	// Modules own their sidebar: routes with a contributing module get one
	// (workspace name plus that module's section); everything else renders
	// full-width, so a module can simply opt out.
	const showSidebar = $derived(
		sidebarModuleFor(page.url.pathname, new Set(plugins.activeIds)) !== null,
	);

	onMount(() => {
		let returnCount = 0;
		const signIn = getNouraClient().sync.signIn;
		const unsubscribeSignIn = signIn.subscribe((value) => {
			if (value.returnCount > returnCount) {
				returnCount = value.returnCount;
				settingsDialog.showSync();
			}
		});
		void signIn.initialize();
		let disposed = false;
		let unlistenClose: (() => void) | undefined;
		void workspace.init();
		void plugins.init();
		const host = createTauriHostLifecycle();
		void installPendingDraftCloseGuard(host, () =>
			saveBeforeLeaving(() => host.forceClose()),
		).then((unlisten) => {
			if (disposed) unlisten();
			else unlistenClose = unlisten;
		});
		return () => {
			disposed = true;
			unsubscribeSignIn();
			unlistenClose?.();
		};
	});
</script>

<ModeWatcher />

<svelte:head>
	<title>{workspace.name}</title>
</svelte:head>

<svelte:window onkeydown={handleKeydown} />

<div
	class="flex h-svh min-h-0 flex-col overflow-hidden"
	style:--content-text-size={preferences.textSizeValue}
	style:--content-max-width={preferences.lineWidthValue}
>
	<header
		class="flex h-(--app-titlebar-height) shrink-0 items-center border-b border-sidebar-border bg-sidebar"
		data-tauri-drag-region
	>
		{#if workspace.isReady && !showSidebar}
			<div
				class="ml-14 flex h-full w-56 shrink-0 items-center pr-2 pl-6"
				data-tauri-drag-region
			>
				<WorkspaceSwitcher />
			</div>
		{/if}
	</header>

	{#if workspace.isReady}
		<Sidebar.Provider
			class="min-h-0 flex-1 overflow-hidden"
			style="--sidebar-width: 14rem;"
		>
			<AppRail />
			{#if showSidebar}
				<AppSidebar />
			{/if}
			<main class="flex min-w-0 flex-1 flex-col">
				<TabsBar />
				{@render children()}
			</main>
			{#if Palette}
				<Palette />
			{/if}
		</Sidebar.Provider>
	{:else if !workspace.initialized || workspace.isLoading}
		<WorkspaceLoading />
	{:else if workspace.isIdle}
		<WorkspaceOnboarding />
	{:else}
		<WorkspaceOnboarding
			problem={workspace.error ?? 'The workspace folder can’t be read.'}
		/>
	{/if}
</div>

{#if SettingsModal}
	<SettingsModal />
{/if}
<Toaster />
<AppUpdateNotice />
