<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { getSettingsDialog } from '$lib/settings.svelte';
	const settings = getSettingsDialog();
	onMount(() => {
		void goto('/inbox', { replaceState: true }).then(() => settings.show());
	});
</script>

<p class="p-6 text-sm text-muted-foreground" role="status">Opening settings…</p>
