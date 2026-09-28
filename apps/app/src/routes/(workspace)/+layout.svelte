<script lang="ts">
	import { onMount, type Component, type Snippet } from 'svelte';
	import { getAppPlatform } from '$lib/platform';
	let { children } = $props<{ children: Snippet }>();
	const shells = {
		native: () => import('$lib/components/native-workspace-shell.svelte'),
		browser: () => import('$lib/components/browser-workspace-shell.svelte'),
	};
	let Shell = $state<Component<{ children?: Snippet }> | null>(null);
	let error = $state('');
	onMount(() => {
		let disposed = false;
		const platform = getAppPlatform();
		// Look the loader up from a table. The minifier folds an if/else or
		// ternary of two dynamic imports into one preload call that lists only
		// the second branch's stylesheets, so the native shell lost its CSS.
		const load =
			shells[platform && platform !== 'web' ? 'native' : 'browser']();
		void load
			.then((module) => {
				if (!disposed)
					Shell = module.default as Component<{ children?: Snippet }>;
			})
			.catch(() => {
				if (!disposed) error = 'Could not load the workspace. Reload to retry.';
			});
		return () => {
			disposed = true;
		};
	});
</script>

{#if Shell}
	<Shell {children} />
{:else}
	<p role="status" class="p-8">{error || 'Loading workspace…'}</p>
{/if}
