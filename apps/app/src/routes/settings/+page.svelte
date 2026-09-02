<script lang="ts">
	import { getNouraClient, workspace, diagnostics } from '$lib/state.svelte';
	import { plugins } from '$lib/plugins.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import GearSix from 'phosphor-svelte/lib/GearSix';

	let providers = $state<
		Awaited<
			ReturnType<ReturnType<typeof getNouraClient>['ai']['listProviders']>
		>
	>([]);
	let loadingProviders = $state(false);
	let toggling = $state('');
	const firstParty = ['folders', 'notes', 'tasks', 'calendar', 'projects'];

	onMount(async () => {
		if (!browser) return;
		loadingProviders = true;
		providers = await getNouraClient()
			.ai.listProviders()
			.catch(() => []);
		loadingProviders = false;
		await plugins.init();
	});

	async function rebuildIndex() {
		try {
			await getNouraClient().workspaces.rebuildIndex();
			await workspace.refresh();
		} catch {}
	}

	async function setPluginEnabled(pluginId: string, enabled: boolean) {
		toggling = pluginId;
		try {
			await plugins.setEnabled(pluginId, enabled);
			toast.success(enabled ? `Enabled ${pluginId}` : `Disabled ${pluginId}`);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not update plugins',
			);
			await plugins.sync();
		} finally {
			toggling = '';
		}
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
			<h2 class="text-sm font-medium">Plugins</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Every change rewrites workspace.yaml in your folder
			</p>
			<Separator class="my-4" />
			{#if workspace.isIdle}
				<p class="text-sm text-muted-foreground">
					Open a workspace to manage plugins.
				</p>
			{:else}
				<div class="flex flex-col gap-2">
					{#each firstParty as pluginId (pluginId)}
						{@const active = plugins.activeManifests.find(
							(m) => m.id === pluginId,
						)}
						<div
							class="flex items-center justify-between rounded-lg border border-border p-3"
						>
							<div class="min-w-0">
								<div class="flex items-center gap-2">
									<span class="text-sm font-medium">
										{active?.name ?? pluginId}
									</span>
									{#if active}
										<Badge variant="secondary" class="text-xs"
											>{active.version}</Badge
										>
									{:else if plugins.enabledIds.includes(pluginId)}
										<Badge variant="outline" class="text-xs"
											>Pending restart</Badge
										>
									{/if}
								</div>
								<div class="mt-1 flex flex-wrap gap-1">
									{#each active?.capabilities ?? [] as capability (capability)}
										<Badge variant="outline" class="text-[10px]"
											>{capability}</Badge
										>
									{/each}
								</div>
							</div>
							<Switch
								checked={plugins.enabledIds.includes(pluginId)}
								disabled={toggling !== ''}
								aria-label="Toggle {active?.name ?? pluginId} plugin"
								onCheckedChange={(checked) =>
									setPluginEnabled(pluginId, checked)}
							/>
						</div>
					{/each}
				</div>
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
