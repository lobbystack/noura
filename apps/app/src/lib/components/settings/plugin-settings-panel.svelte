<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { toast } from 'svelte-sonner';
	import type { PluginSettingsModel } from '$lib/plugin-settings-model';
	import { Button } from '$lib/components/ui/button';
	import { Switch } from '$lib/components/ui/switch';
	import { Badge } from '$lib/components/ui/badge';
	import { Separator } from '$lib/components/ui/separator';
	import PluginPlatforms from './plugin-platforms.svelte';
	import { platformLabels } from '$lib/platform';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Calendar from 'phosphor-svelte/lib/Calendar';
	import Kanban from 'phosphor-svelte/lib/Kanban';
	import CaretRight from 'phosphor-svelte/lib/CaretRight';
	import ArrowLeft from 'phosphor-svelte/lib/ArrowLeft';
	let {
		plugins,
		workspaceReady,
		configuration,
	}: {
		plugins: PluginSettingsModel;
		workspaceReady: boolean;
		configuration?: Snippet<[string]>;
	} = $props();

	const catalog = [
		{
			id: 'ai',
			name: 'AI',
			description: 'AI chat and provider settings',
			icon: Sparkle,
		},
		{
			id: 'notes',
			name: 'Notes',
			description: 'Markdown notes',
			icon: NotePencil,
		},
		{
			id: 'tasks',
			name: 'Tasks',
			description: 'Tasks and due dates',
			icon: Checks,
		},
		{
			id: 'projects',
			name: 'Projects',
			description: 'Project notes and task boards',
			icon: Kanban,
		},
		{
			id: 'calendar',
			name: 'Calendar',
			description: 'Tasks by date',
			icon: Calendar,
		},
		{
			id: 'folders',
			name: 'Folders',
			description: 'Workspace files',
			icon: FolderOpen,
		},
	];
	let selectedId = $state<string | null>(null);
	let toggling = $state('');
	let error = $state('');
	const selected = $derived(catalog.find((plugin) => plugin.id === selectedId));
	const orderedCatalog = $derived.by(() =>
		plugins.orderedPluginIds.flatMap((id) => {
			const plugin = catalog.find((candidate) => candidate.id === id);
			return plugin ? [plugin] : [];
		}),
	);
	const manifest = $derived(
		plugins.catalog.find((plugin) => plugin.id === selectedId),
	);

	onMount(() => {
		void plugins.sync();
	});

	async function setEnabled(id: string, enabled: boolean) {
		toggling = id;
		error = '';
		try {
			await plugins.setEnabled(id, enabled);
		} catch (cause) {
			error =
				cause instanceof Error
					? cause.message
					: 'Could not update plugin. Try again.';
			toast.error(error);
		} finally {
			toggling = '';
		}
	}
</script>

<p class="mb-4 text-sm text-muted-foreground">
	Current platform: {plugins.platform
		? platformLabels[plugins.platform]
		: 'Unknown native platform'}.
	{#if !workspaceReady}Open a workspace to manage plugins.{/if}
	{#if plugins.platform === 'web'}Browser plugins have objects, commands and
		events only. Native-only features remain unavailable.{/if}
</p>
{#if error || plugins.lastError}<p
		role="alert"
		class="text-sm text-destructive"
	>
		{error || plugins.lastError}
	</p>{/if}
{#if !plugins.synced}<p role="status" class="text-sm text-muted-foreground">
		Loading plugins…
	</p>
{/if}
{#if selected}
	<div class="flex flex-col gap-6">
		<div>
			<Button
				variant="ghost"
				size="sm"
				onclick={() => {
					selectedId = null;
				}}><ArrowLeft data-icon="inline-start" />Plugins</Button
			>
		</div>
		<div class="flex items-start gap-4">
			<div class="flex size-11 shrink-0 items-center justify-center">
				<selected.icon class="size-5" />
			</div>
			<div class="flex min-w-0 flex-1 flex-col gap-1">
				<h2 class="text-base font-semibold">{selected.name}</h2>
				<p class="text-sm text-muted-foreground">{selected.description}</p>
				<PluginPlatforms {manifest} />
			</div>
			<Switch
				aria-label="Enable {selected.name}"
				bind:checked={
					() => plugins.enabledIds.includes(selected.id),
					(enabled) => setEnabled(selected.id, enabled)
				}
				aria-describedby="selected-plugin-availability"
				disabled={toggling !== '' || !!plugins.unavailableReason(selected.id)}
			/>
		</div>
		<p id="selected-plugin-availability" class="text-xs text-muted-foreground">
			{plugins.unavailableReason(selected.id) ??
				(plugins.isEnabled(selected.id)
					? 'Active on this device.'
					: 'Disabled.')}
			{#if plugins.enabledIds.includes(selected.id) && !plugins.isSupported(selected.id)}Enabled
				in workspace preferences, but inactive on this device.{/if}
			{manifest ? ` · Version ${manifest.version}` : ''}
		</p>
		<Separator />
		{#if plugins.unavailableReason(selected.id)}
			<p class="text-sm text-muted-foreground">
				Plugin actions and configuration are unavailable here. Workspace
				preferences are unchanged.
			</p>
		{:else if configuration}
			{@render configuration(selected.id)}
		{:else}
			<p class="text-sm text-muted-foreground">No additional settings.</p>
		{/if}
		{#if manifest?.capabilities.length}
			<details class="text-sm">
				<summary class="cursor-pointer text-muted-foreground"
					>Plugin capabilities</summary
				>
				<div class="flex flex-wrap gap-2 pt-3">
					{#each manifest.capabilities as capability (capability)}<Badge
							variant="outline">{capability}</Badge
						>{/each}
				</div>
			</details>
		{/if}
	</div>
{:else}
	<div class="flex flex-col">
		<div class="flex flex-col divide-y divide-border" role="list">
			{#each orderedCatalog as plugin (plugin.id)}
				{@const reason = plugins.unavailableReason(plugin.id)}
				<div role="listitem" class="flex items-center gap-4 py-4">
					<button
						type="button"
						class="group flex min-w-0 flex-1 items-center gap-4 rounded-lg text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
						onclick={() => {
							selectedId = plugin.id;
						}}
						aria-label="{plugin.name} settings"
					>
						<span class="flex size-10 shrink-0 items-center justify-center"
							><plugin.icon class="size-5 text-muted-foreground" /></span
						>
						<span class="flex min-w-0 flex-1 flex-col gap-1"
							><span class="font-medium">{plugin.name}</span><span
								class="text-sm leading-relaxed text-muted-foreground"
								>{plugin.description}</span
							><PluginPlatforms
								manifest={plugins.catalog.find(
									(manifest) => manifest.id === plugin.id,
								)}
							/>
							<span
								id="plugin-availability-{plugin.id}"
								class="text-xs text-muted-foreground"
								>{reason ??
									(plugins.isEnabled(plugin.id)
										? 'Active on this device.'
										: 'Disabled.')}{#if plugins.enabledIds.includes(plugin.id) && !plugins.isSupported(plugin.id)}
									Enabled in workspace preferences, but inactive on this device.{/if}</span
							></span
						>
						<CaretRight class="size-4 shrink-0 text-muted-foreground" />
					</button>
					<Switch
						aria-label="Enable {plugin.name}"
						bind:checked={
							() => plugins.enabledIds.includes(plugin.id),
							(enabled) => setEnabled(plugin.id, enabled)
						}
						aria-describedby="plugin-availability-{plugin.id}"
						disabled={toggling !== '' || !!reason}
					/>
				</div>
			{/each}
		</div>
	</div>
{/if}
