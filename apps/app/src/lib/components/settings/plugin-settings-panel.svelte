<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { toast } from 'svelte-sonner';
	import type { PluginSettingsModel } from '$lib/plugin-settings-model';
	import { Button } from '$lib/components/ui/button';
	import { Switch } from '$lib/components/ui/switch';
	import { Badge } from '$lib/components/ui/badge';
	import { Separator } from '$lib/components/ui/separator';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Calendar from 'phosphor-svelte/lib/Calendar';
	import Kanban from 'phosphor-svelte/lib/Kanban';
	import ArrowsClockwise from 'phosphor-svelte/lib/ArrowsClockwise';
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
			id: 'sync',
			name: 'Sync',
			description: 'Encrypted sync and live editing across devices',
			icon: ArrowsClockwise,
		},
	];
	// Files and notes are part of noura itself, so they have no switch here.
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

	/** Plugins that ask before they turn off. */
	const disableConfirmations: Record<
		string,
		{ title: string; description: string; action: string }
	> = {
		sync: {
			title: 'Turn off Sync?',
			description:
				'Sync stops on this device. Your files and sync setup are kept.',
			action: 'Turn off',
		},
	};
	let confirming = $state<string | null>(null);
	const confirmation = $derived(
		confirming ? disableConfirmations[confirming] : undefined,
	);

	function requestEnabled(id: string, enabled: boolean) {
		if (!enabled && disableConfirmations[id]) {
			confirming = id;
			return;
		}
		void setEnabled(id, enabled);
	}

	function confirmDisable() {
		const id = confirming;
		confirming = null;
		if (id) void setEnabled(id, false);
	}

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

{#if !workspaceReady || plugins.platform === 'web'}
	<p class="mb-4 text-sm text-muted-foreground">
		{#if !workspaceReady}Open a workspace to manage plugins.{/if}
		{#if plugins.platform === 'web'}In the browser, some plugin features only
			work in the desktop app.{/if}
	</p>
{/if}
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
			</div>
			<Switch
				aria-label="Enable {selected.name}"
				bind:checked={
					() => plugins.enabledIds.includes(selected.id),
					(enabled) => requestEnabled(selected.id, enabled)
				}
				aria-describedby={plugins.unavailableReason(selected.id)
					? 'selected-plugin-availability'
					: undefined}
				disabled={toggling !== '' || !!plugins.unavailableReason(selected.id)}
			/>
		</div>
		{#if plugins.unavailableReason(selected.id)}
			<p
				id="selected-plugin-availability"
				class="text-xs text-muted-foreground"
			>
				{plugins.unavailableReason(selected.id)}
			</p>
		{/if}
		<Separator />
		{#if configuration && !plugins.unavailableReason(selected.id)}
			{@render configuration(selected.id)}
		{/if}
		{#if manifest?.capabilities.length}
			<details class="text-sm">
				<summary class="cursor-pointer text-muted-foreground"
					>What this plugin can use · version {manifest.version}</summary
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
							>
							{#if reason}
								<span
									id="plugin-availability-{plugin.id}"
									class="text-xs text-muted-foreground">{reason}</span
								>
							{/if}
						</span>
						<CaretRight class="size-4 shrink-0 text-muted-foreground" />
					</button>
					<Switch
						aria-label="Enable {plugin.name}"
						bind:checked={
							() => plugins.enabledIds.includes(plugin.id),
							(enabled) => requestEnabled(plugin.id, enabled)
						}
						aria-describedby={reason
							? `plugin-availability-${plugin.id}`
							: undefined}
						disabled={toggling !== '' || !!reason}
					/>
				</div>
			{/each}
		</div>
	</div>
{/if}

<AlertDialog.Root
	open={confirmation !== undefined}
	onOpenChange={(open) => {
		if (!open) confirming = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{confirmation?.title}</AlertDialog.Title>
			<AlertDialog.Description
				>{confirmation?.description}</AlertDialog.Description
			>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={confirmDisable}
				>{confirmation?.action}</AlertDialog.Action
			>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
