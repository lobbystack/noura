<script lang="ts">
	import { onMount, type Component, type Snippet } from 'svelte';
	import { getAppPlatform } from '$lib/platform';
	let { children } = $props<{ children: Snippet }>();
	let Shell = $state<Component<{ children?: Snippet }> | null>(null);
	let error = $state('');
	onMount(() => {
		let disposed = false;
		const platform = getAppPlatform();
		const load =
			platform && platform !== 'web'
				? import('$lib/components/native-workspace-shell.svelte')
				: import('$lib/components/browser-workspace-shell.svelte');
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
