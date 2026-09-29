<script lang="ts">
	import { onMount, type Component, type Snippet } from 'svelte';
	import { getAppPlatform } from '$lib/platform';
	import WorkspaceLoading from '$lib/components/workspace-loading.svelte';

	let { children } = $props<{ children: Snippet }>();
	const shells = {
		native: () => import('$lib/components/native-workspace-shell.svelte'),
		browser: () => import('$lib/components/browser-workspace-shell.svelte'),
	};
	const platform = getAppPlatform();
	const native = Boolean(platform && platform !== 'web');
	let Shell = $state<Component<{ children?: Snippet }> | null>(null);
	let error = $state('');
	onMount(() => {
		let disposed = false;
		// Look the loader up from a table. The minifier folds an if/else or
		// ternary of two dynamic imports into one preload call that lists only
		// the second branch's stylesheets, so the native shell lost its CSS.
		const load = shells[native ? 'native' : 'browser']();
		void load
			.then((module) => {
				if (!disposed)
					Shell = module.default as Component<{ children?: Snippet }>;
			})
			.catch(() => {
				if (!disposed) error = 'noura could not start. Reload to try again.';
			});
		return () => {
			disposed = true;
		};
	});
</script>

{#if Shell}
	<Shell {children} />
{:else if error}
	<p role="alert" class="p-8 text-sm">{error}</p>
{:else}
	<!-- Matches the shell's own loading screen, title bar included, so the
	hand-off is invisible. -->
	<div class="flex h-svh flex-col">
		{#if native}
			<div
				class="h-(--app-titlebar-height) shrink-0 border-b border-sidebar-border bg-sidebar"
				data-tauri-drag-region
			></div>
		{/if}
		<WorkspaceLoading />
	</div>
{/if}
