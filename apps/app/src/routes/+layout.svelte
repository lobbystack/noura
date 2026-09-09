<script lang="ts">
	import '../app.css';
	import { RouteSidebar, setRouteSidebar } from '$lib/route-sidebar.svelte';
	setRouteSidebar(new RouteSidebar());
	import TabsBar from '$lib/components/tabs-bar.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import AppRail from '$lib/components/app-rail.svelte';
	import AppSidebar from '$lib/components/app-sidebar.svelte';
	import WorkspaceSwitcher from '$lib/components/workspace-switcher.svelte';
	import WorkspaceOnboarding from '$lib/components/workspace-onboarding.svelte';
	import { workspace } from '$lib/state.svelte';
	import { aiChats } from '$lib/ai/chat-store.svelte';
	import { sidebarModuleFor } from '$lib/sidebar-modules';
	import { plugins, PLUGIN_ROUTES } from '$lib/plugins.svelte';
	import {
		flushPendingDrafts,
		hasPendingDrafts,
	} from '$lib/editor/pending-drafts.svelte';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	import { beforeNavigate, goto } from '$app/navigation';
	import { page } from '$app/stores';
	import {
		createTauriHostLifecycle,
		installPendingDraftCloseGuard,
	} from '@noura/workspace';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Toaster } from '$lib/components/ui/sonner/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import CommandPalette from '$lib/components/command-palette.svelte';
	import type { Snippet } from 'svelte';

	let { children } = $props<{ children: Snippet }>();

	let allowedNavigation: string | null = null;
	$effect(() => {
		const id = workspace.state?.workspaceId;
		tabsStore.setWorkspace(id);
	});

	beforeNavigate((navigation) => {
		const destination = navigation.to?.url;
		if (destination?.href === allowedNavigation) {
			allowedNavigation = null;
			return;
		}
		if (!hasPendingDrafts()) return;
		navigation.cancel();
		void flushPendingDrafts().then((saved) => {
			if (!saved || !destination) return;
			allowedNavigation = destination.href;
			void goto(destination, {
				replaceState: navigation.type === 'popstate',
			});
		});
	});

	// Turned-off modules genuinely simplify the workspace: routes backed by a
	// disabled plugin fall back to the inbox dashboard instead of rendering a
	// dead surface.
	$effect(() => {
		if (!browser || !plugins.synced) return;
		const path = $page.url.pathname;
		for (const [pluginId, route] of PLUGIN_ROUTES) {
			if (path.startsWith(route) && !plugins.isEnabled(pluginId)) {
				void goto('/inbox', { replaceState: true });
				return;
			}
		}
	});

	// The engine broadcasts workspace:ready before the host event bridge
	// subscribes, so create/open flows would miss it. Reconcile plugins from
	// the reactive workspace state on every settle (ready, idle, failed) so
	// toggles and navigation always match workspace.yaml.
	$effect(() => {
		if (!browser) return;
		const phase = workspace.state?.phase;
		if (phase === 'ready' || phase === 'idle' || phase === 'failed')
			void plugins.sync();
	});

	// AI providers are global, while chats are workspace-scoped. Reconcile both
	// projections from settled workspace state in case the host event bridge
	// subscribed after a workspace transition.
	$effect(() => {
		if (!browser) return;
		const phase = workspace.state?.phase;
		if (phase === 'ready' || phase === 'idle' || phase === 'failed')
			void aiChats.refresh();
	});

	// Modules own their sidebar: routes with a contributing module get one
	// (workspace name plus that module's section); everything else renders
	// full-width — a module can simply opt out.
	const showSidebar = $derived(
		sidebarModuleFor($page.url.pathname, new Set(plugins.enabledIds)) !== null,
	);

	onMount(() => {
		let disposed = false;
		let unlistenClose: (() => void) | undefined;
		if (browser) {
			void workspace.init();
			void plugins.init();
			void aiChats.init();
			void installPendingDraftCloseGuard(
				createTauriHostLifecycle(),
				flushPendingDrafts,
			).then((unlisten) => {
				if (disposed) unlisten();
				else unlistenClose = unlisten;
			});
		}
		return () => {
			disposed = true;
			unlistenClose?.();
		};
	});
</script>

<svelte:head>
	<title>{workspace.name}</title>
</svelte:head>

<div class="flex h-svh min-h-0 flex-col overflow-hidden">
	<header
		class="flex h-(--app-titlebar-height) shrink-0 items-center border-b border-border/60 bg-background"
		data-tauri-drag-region
	>
		<div class="w-[4.75rem] shrink-0" data-tauri-drag-region></div>
		<WorkspaceSwitcher />
	</header>

	{#if browser && workspace.isReady}
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
			<CommandPalette />
		</Sidebar.Provider>
	{:else if !browser || !workspace.initialized || workspace.isLoading}
		<div class="flex min-h-0 flex-1 items-center justify-center">
			<div class="flex flex-col items-center gap-3">
				<Spinner class="size-6" />
				<p class="text-sm text-muted-foreground">Loading workspace…</p>
			</div>
		</div>
	{:else if workspace.isIdle}
		<WorkspaceOnboarding />
	{:else}
		<Empty.Root class="min-h-0">
			<Empty.Header>
				<Empty.Title>Workspace unavailable</Empty.Title>
				<Empty.Description
					>{workspace.error ??
						'Noura could not read the current workspace state.'}</Empty.Description
				>
			</Empty.Header>
			<Empty.Content>
				<Button onclick={() => workspace.refresh()}>Retry</Button>
			</Empty.Content>
		</Empty.Root>
	{/if}
</div>

<Toaster />
