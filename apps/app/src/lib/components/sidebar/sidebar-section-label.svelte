<script lang="ts">
	import type { Snippet } from 'svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { isSectionCollapsed, toggleSection } from '$lib/preferences.svelte';
	import { cn } from '$lib/utils.js';
	import CaretRight from 'phosphor-svelte/lib/CaretRight';

	/**
	 * A sidebar group heading that folds its section. Every section starts
	 * open; the folded ones are remembered on this device.
	 */
	let {
		section,
		class: className,
		children,
	}: {
		/** Stable key for remembering the folded state. */
		section: string;
		class?: string;
		children: Snippet;
	} = $props();

	const collapsed = $derived(isSectionCollapsed(section));
</script>

<Sidebar.SidebarGroupLabel class={cn('gap-1 pl-2', className)}>
	{#snippet child({ props })}
		<button
			{...props}
			type="button"
			aria-expanded={!collapsed}
			onclick={() => toggleSection(section)}
		>
			<CaretRight
				class={cn(
					'size-3! transition-transform duration-150',
					!collapsed && 'rotate-90',
				)}
			/>
			{@render children()}
		</button>
	{/snippet}
</Sidebar.SidebarGroupLabel>
