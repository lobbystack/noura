<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { workspaceTree } from '$lib/workspace-tree.svelte';

	let { workspaceId }: { workspaceId: string | null | undefined } = $props();

	onMount(() => {
		if (browser) void workspaceTree.start(workspaceId);
	});
</script>
