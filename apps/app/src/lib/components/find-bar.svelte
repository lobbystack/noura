<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import type { FindController } from '@noura/editor/types';
	import * as InputGroup from '$lib/components/ui/input-group/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import CaretUp from 'phosphor-svelte/lib/CaretUp';
	import CaretDown from 'phosphor-svelte/lib/CaretDown';
	import X from 'phosphor-svelte/lib/X';

	let { controller }: { controller: FindController } = $props();

	let text = $state(untrack(() => controller.query()));
	let matches = $state.raw(untrack(() => controller.matches()));
	let input = $state<HTMLInputElement | null>(null);

	$effect(() =>
		controller.subscribe(() => {
			matches = controller.matches();
			const query = controller.query();
			if (query !== text) text = query;
		}),
	);

	/** Put the caret in the field with its text selected. */
	export function focus() {
		input?.focus();
		input?.select();
	}

	onMount(focus);

	const count = $derived(
		matches.total === 0
			? text
				? 'No results'
				: ''
			: `${matches.current}/${matches.total}${matches.capped ? '+' : ''}`,
	);

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			if (event.shiftKey) controller.previous();
			else controller.next();
		} else if (event.key === 'Escape') {
			event.preventDefault();
			controller.close();
		}
	}
</script>

<div
	role="search"
	class="absolute top-2 right-3 z-20 w-72 rounded-xl bg-popover shadow-md"
>
	<InputGroup.Root class="h-8 bg-popover">
		<InputGroup.Input
			bind:ref={input}
			bind:value={text}
			aria-label="Find in note"
			placeholder="Find in note"
			spellcheck="false"
			autocomplete="off"
			oninput={() => controller.setQuery(text)}
			onkeydown={handleKeydown}
		/>
		<InputGroup.Addon align="inline-end" class="gap-0.5">
			<span
				class="pr-1 text-xs font-normal text-muted-foreground tabular-nums"
				aria-live="polite">{count}</span
			>
			{@render action('Previous match (Shift+Enter)', CaretUp, () =>
				controller.previous(),
			)}
			{@render action('Next match (Enter)', CaretDown, () => controller.next())}
			{@render action('Close (Esc)', X, () => controller.close(), false)}
		</InputGroup.Addon>
	</InputGroup.Root>
</div>

{#snippet action(
	label: string,
	Icon: typeof X,
	run: () => void,
	needsMatches = true,
)}
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<InputGroup.Button
					{...props}
					size="icon-xs"
					aria-label={label}
					disabled={needsMatches && matches.total === 0}
					onclick={run}
				>
					<Icon />
				</InputGroup.Button>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="bottom">{label}</Tooltip.Content>
	</Tooltip.Root>
{/snippet}
