<script lang="ts">
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import type { Component } from 'svelte';

	let {
		icon: Icon,
		title,
		description,
		actionLabel,
		onAction,
	}: {
		icon?: Component;
		title: string;
		description?: string;
		actionLabel?: string;
		onAction?: () => void;
	} = $props();
</script>

<Empty.Root class="flex-1">
	{#if Icon}
		<Empty.Media variant="icon"><Icon /></Empty.Media>
	{/if}
	<Empty.Header>
		<Empty.Title>{title}</Empty.Title>
		{#if description}
			<Empty.Description>{description}</Empty.Description>
		{/if}
	</Empty.Header>
	{#if actionLabel && onAction}
		<Empty.Content>
			<Button onclick={onAction}>{actionLabel}</Button>
		</Empty.Content>
	{/if}
</Empty.Root>
