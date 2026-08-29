<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	let providers = $state<
		Awaited<
			ReturnType<ReturnType<typeof getNouraClient>['ai']['listProviders']>
		>
	>([]);
	let ready = $derived(providers.filter((p) => p.enabled && p.credentialRef));

	onMount(async () => {
		if (browser)
			providers = await getNouraClient()
				.ai.listProviders()
				.catch(() => []);
	});
</script>

<div class="flex h-14 shrink-0 items-center border-b border-border px-6">
	<h1 class="text-sm font-semibold">AI</h1>
	<Badge variant="secondary" class="ml-2 h-5 px-1.5 text-[0.6rem]"
		>{ready.length} ready</Badge
	>
</div>

{#if ready.length === 0}
	<EmptyState
		icon={Sparkle}
		title="No AI providers configured"
		description="Go to Settings → AI Providers to set up a provider with an API key."
	/>
{:else}
	<div class="flex-1 overflow-y-auto p-6 space-y-2">
		{#each ready as p (p.id)}
			<div
				class="flex items-center justify-between rounded-lg border border-border p-3"
			>
				<div class="flex items-center gap-2">
					<Sparkle class="size-4" />
					<span class="text-sm font-medium">{p.displayName}</span>
				</div>
				<Badge variant="secondary">{p.model}</Badge>
			</div>
		{/each}
	</div>
{/if}
