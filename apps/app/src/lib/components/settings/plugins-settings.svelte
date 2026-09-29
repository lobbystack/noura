<script lang="ts">
	import { plugins } from '$lib/plugins.svelte';
	import { workspace } from '$lib/state.svelte';
	import { Button } from '$lib/components/ui/button';
	import PluginSettingsPanel from './plugin-settings-panel.svelte';

	let {
		onopenai,
		onopensync,
	}: { onopenai: () => void; onopensync: () => void } = $props();
</script>

<PluginSettingsPanel {plugins} workspaceReady={workspace.isReady}>
	{#snippet configuration(id: string)}
		{#if id === 'ai'}
			<div class="flex flex-wrap items-center justify-between gap-3">
				<p class="text-sm text-muted-foreground">
					Providers and permissions have their own section.
				</p>
				<Button variant="outline" size="sm" onclick={onopenai}
					>Open AI settings</Button
				>
			</div>
		{:else if id === 'sync'}
			<div class="flex flex-wrap items-center justify-between gap-3">
				<p class="text-sm text-muted-foreground">
					Manage devices, people, and recovery kits in Sync settings.
				</p>
				<Button
					variant="outline"
					size="sm"
					disabled={!plugins.isEnabled('sync')}
					onclick={onopensync}>Open sync settings</Button
				>
			</div>
		{/if}
	{/snippet}
</PluginSettingsPanel>
