<script lang="ts">
	import { goto } from '$app/navigation';
	import { tabsStore } from '$lib/tabs.svelte';
	import { cn } from '$lib/utils.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import X from 'phosphor-svelte/lib/X';
	import PushPin from 'phosphor-svelte/lib/PushPin';

	function activate(id: string) {
		tabsStore.setActive(id);
		const tab = tabsStore.active;
		if (!tab) {
			void goto('/notes');
			return;
		}
		const href =
			tab.href ??
			(tab.objectId.startsWith('raw:')
				? `/notes?raw=${encodeURIComponent(tab.objectId.slice(4))}`
				: `/${tab.objectType === 'task' ? 'tasks' : tab.objectType === 'project' ? 'projects' : 'notes'}?selected=${encodeURIComponent(tab.objectId)}`);
		void goto(href);
	}
	let { onClose }: { onClose?: (id: string) => void } = $props();
</script>

{#if tabsStore.tabs.length > 0}
	<div
		class="flex h-9 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-border bg-muted/40 px-2"
	>
		{#each tabsStore.tabs as tab (tab.id)}
			{@const isActive = tabsStore.activeId === tab.id}
			<div
				class={cn(
					'group flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-sm transition-colors',
					isActive
						? 'bg-background text-foreground shadow-sm ring-1 ring-border'
						: 'text-muted-foreground hover:bg-background hover:text-foreground',
				)}
			>
				{#if !tab.pinned}
					<span class="size-1.5 rounded-full bg-muted-foreground/50"></span>
				{/if}
				<button
					class="max-w-40 truncate"
					onclick={() => activate(tab.id)}
					ondblclick={() => tabsStore.pin(tab.id)}
				>
					{tab.title}
				</button>
				{#if !tab.pinned}
					<Tooltip.Root>
						<Tooltip.Trigger>
							{#snippet child({ props })}
								<button
									{...props}
									class="hidden size-4 items-center justify-center rounded-sm text-muted-foreground/70 hover:text-foreground group-hover:flex"
									aria-label={`Pin ${tab.title}`}
									onclick={() => tabsStore.pin(tab.id)}
								>
									<PushPin />
								</button>
							{/snippet}
						</Tooltip.Trigger>
						<Tooltip.Content side="bottom">Pin tab</Tooltip.Content>
					</Tooltip.Root>
				{/if}
				<button
					aria-label={`Close ${tab.title}`}
					class="size-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground/50 hover:text-foreground"
					onclick={() => {
						const wasActive = tabsStore.activeId === tab.id;
						tabsStore.close(tab.id);
						if (wasActive) activate(tabsStore.activeId ?? '');
						onClose?.(tab.id);
					}}
				>
					<X />
				</button>
			</div>
		{/each}
	</div>
{/if}
