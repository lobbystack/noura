<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		title,
		description,
		canDiscard,
		onretry,
		onkeep,
		ondiscard,
	}: {
		title: string;
		description: string;
		canDiscard: boolean;
		onretry: () => void;
		onkeep: () => void;
		ondiscard: () => void;
	} = $props();
</script>

<div
	class="flex w-(--width) flex-col gap-3 rounded-lg border border-border bg-popover p-4 text-sm text-popover-foreground shadow-lg"
	role="alert"
>
	<div class="flex gap-2">
		<Warning class="mt-0.5 size-4 shrink-0 text-destructive" />
		<div class="flex min-w-0 flex-col gap-1">
			<p class="font-medium">{title}</p>
			<p class="text-xs text-muted-foreground">{description}</p>
		</div>
	</div>
	<div class="flex flex-wrap justify-end gap-2">
		{#if canDiscard}
			<Button variant="ghost" size="sm" onclick={ondiscard}
				>Discard changes</Button
			>
		{/if}
		<Button variant="outline" size="sm" onclick={onkeep}>Keep editing</Button>
		<Button size="sm" onclick={onretry}>Try again</Button>
	</div>
</div>
