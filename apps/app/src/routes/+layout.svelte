<script lang="ts">
	import '../app.css';
	import type { Snippet } from 'svelte';
	let { children } = $props<{ children: Snippet }>();
</script>

{@render children()}
