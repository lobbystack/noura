<script lang="ts">
	import '../app.css';
	import AppRail from '$lib/components/app-rail.svelte';
	import AppSidebar from '$lib/components/app-sidebar.svelte';
	import WorkspaceOnboarding from '$lib/components/workspace-onboarding.svelte';
	import { workspace } from '$lib/state.svelte';
	import {
		flushPendingDrafts,
		hasPendingDrafts,
	} from '$lib/editor/pending-drafts.svelte';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	import { beforeNavigate, goto } from '$app/navigation';
	import {
		createTauriHostLifecycle,
		installPendingDraftCloseGuard,
	} from '@noura/workspace';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Toaster } from '$lib/components/ui/sonner/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import type { Snippet } from 'svelte';

	let { children } = $props<{ children: Snippet }>();

	let allowedNavigation: string | null = null;

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

	onMount(() => {
		let disposed = false;
		let unlistenClose: (() => void) | undefined;
		if (browser) {
			void workspace.init();
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
	<title>Noura</title>
</svelte:head>

{#if browser && workspace.isReady}
	<Sidebar.Provider
		class="h-svh min-h-0 overflow-hidden"
		style="--sidebar-width: 14rem;"
	>
		<AppRail />
		<AppSidebar />
		<main class="flex min-w-0 flex-1 flex-col">
			{@render children()}
		</main>
	</Sidebar.Provider>
	<Toaster />
{:else if !browser || !workspace.initialized || workspace.isLoading}
	<div class="flex h-svh w-full items-center justify-center">
		<div class="flex flex-col items-center gap-3">
			<Spinner class="size-6" />
			<p class="text-sm text-muted-foreground">Loading workspace…</p>
		</div>
	</div>
{:else if workspace.isIdle}
	<WorkspaceOnboarding />
{:else}
	<Empty.Root class="min-h-svh">
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
