<script lang="ts">
	import {
		supportsPlatform,
		type PluginManifest,
		type PluginPlatform,
	} from '@noura/plugin-sdk';
	import Desktop from 'phosphor-svelte/lib/Desktop';
	import DeviceMobile from 'phosphor-svelte/lib/DeviceMobile';
	import Globe from 'phosphor-svelte/lib/Globe';
	import { platformLabels } from '$lib/platform';

	let { manifest }: { manifest: PluginManifest | undefined } = $props();
	const platforms = [
		{ id: 'desktop', icon: Desktop },
		{ id: 'mobile', icon: DeviceMobile },
		{ id: 'web', icon: Globe },
	] satisfies { id: PluginPlatform; icon: typeof Desktop }[];
</script>

<span class="flex flex-wrap gap-3 text-xs text-muted-foreground">
	{#if manifest}
		{#each platforms as platform (platform.id)}
			{@const supported = supportsPlatform(manifest, platform.id)}
			<span
				class="inline-flex items-center gap-1"
				title="{platformLabels[platform.id]}: {supported
					? 'Supported'
					: 'Not supported'}"
			>
				<platform.icon class="size-4" aria-hidden="true" />
				<span
					>{platformLabels[platform.id]}: {supported
						? 'Supported'
						: 'Not supported'}</span
				>
			</span>
		{/each}
	{:else}
		Platform support unknown
	{/if}
</span>
