<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
</script>

<div class="flex h-14 shrink-0 items-center border-b border-border px-6">
	<h1 class="text-sm font-semibold">Calendar</h1>
</div>

<EmptyState
	icon={CalendarBlank}
	title="No scheduled items"
	description="Items with dates appear here."
/>
