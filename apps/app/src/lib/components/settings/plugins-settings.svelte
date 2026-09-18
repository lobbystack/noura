<script lang="ts">
	import { plugins } from '$lib/plugins.svelte';
	import { workspace } from '$lib/state.svelte';
	import PluginSettingsPanel from './plugin-settings-panel.svelte';
	import AiSettings from './ai-settings.svelte';
</script>

<PluginSettingsPanel {plugins} workspaceReady={workspace.isReady}>
	{#snippet configuration(id: string)}
		{#if id === 'ai'}<AiSettings />{:else}<p
				class="text-sm text-muted-foreground"
			>
				No additional settings.
			</p>{/if}
	{/snippet}
</PluginSettingsPanel>
