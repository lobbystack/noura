<script lang="ts">
	import type { Snippet } from 'svelte';
	import WorkspaceShell from '$lib/components/workspace-shell.svelte';

	let { children } = $props<{ children: Snippet }>();
</script>

<!-- The desktop app and the browser share this shell. -->
<WorkspaceShell {children} />
