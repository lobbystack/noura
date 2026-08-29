<script lang="ts">
	import { getNouraClient, workspace, diagnostics } from '$lib/state.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import GearSix from 'phosphor-svelte/lib/GearSix';

	let providers = $state<
		Awaited<
			ReturnType<ReturnType<typeof getNouraClient>['ai']['listProviders']>
		>
	>([]);
	let loadingProviders = $state(false);

	onMount(async () => {
		if (!browser) return;
		loadingProviders = true;
		providers = await getNouraClient()
			.ai.listProviders()
			.catch(() => []);
		loadingProviders = false;
	});

	async function rebuildIndex() {
		try {
			await getNouraClient().workspaces.rebuildIndex();
			await workspace.refresh();
		} catch {}
	}
</script>

<div class="flex h-14 shrink-0 items-center border-b border-border px-6">
	<h1 class="text-sm font-semibold">Settings</h1>
</div>

<div class="flex-1 overflow-y-auto p-6">
	<div class="mx-auto max-w-2xl space-y-8">
		<section>
			<h2 class="text-sm font-medium">Workspace</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Current workspace and diagnostics
			</p>
			<Separator class="my-4" />
			{#if workspace.state}
				<div class="space-y-3">
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Path</span>
						<span class="truncate font-mono text-xs"
							>{workspace.state.rootPath}</span
						>
					</div>
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Indexed files</span>
						<Badge variant="secondary">{workspace.state.indexedFiles}</Badge>
					</div>
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Issues</span>
						<Badge
							variant={diagnostics.issues.length > 0
								? 'destructive'
								: 'secondary'}>{diagnostics.issues.length}</Badge
						>
					</div>
					<div class="pt-2">
						<Button variant="outline" size="sm" onclick={rebuildIndex}
							>Rebuild index</Button
						>
					</div>
				</div>
			{:else}
				<p class="text-sm text-muted-foreground">No workspace open.</p>
			{/if}
		</section>

		<section>
			<h2 class="text-sm font-medium">AI Providers</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				BYOK — bring your own key
			</p>
			<Separator class="my-4" />
			{#if loadingProviders}
				<p class="text-sm text-muted-foreground">Loading…</p>
			{:else if providers.length === 0}
				<EmptyState
					icon={GearSix}
					title="No AI providers"
					description="Add a provider to enable AI features."
				/>
			{:else}
				<div class="space-y-2">
					{#each providers as p (p.id)}
						<div
							class="flex items-center justify-between rounded-lg border border-border p-3"
						>
							<div>
								<div class="flex items-center gap-2">
									<span class="text-sm font-medium">{p.displayName}</span>
									{#if p.enabled}
										<Badge variant="secondary" class="text-xs">Enabled</Badge>
									{:else}
										<Badge variant="outline" class="text-xs">Disabled</Badge>
									{/if}
								</div>
								<p class="text-xs text-muted-foreground">{p.model}</p>
							</div>
							{#if p.credentialRef}
								<Badge variant="secondary" class="text-xs">Key set</Badge>
							{:else}
								<Badge variant="outline" class="text-xs">No key</Badge>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</section>
	</div>
</div>
