<script lang="ts">
	import type { Note, Project, Task, WorkspaceObject } from '@noura/workspace';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { FileCode } from 'phosphor-svelte';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';

	let {
		open = $bindable(false),
		object = null,
		onclose,
	}: {
		open: boolean;
		object: WorkspaceObject | Task | Note | Project | null;
		onclose?: () => void;
	} = $props();

	const taskProps = $derived(
		object?.properties as Record<string, unknown> | undefined,
	);
	const status = $derived(
		taskProps?.status ? String(taskProps.status) : 'unknown',
	);
	const title = $derived(object?.title ?? 'Untitled');
</script>

<Sheet.Root
	bind:open
	onOpenChange={(v) => {
		if (!v) onclose?.();
	}}
>
	<Sheet.Content side="right" class="w-80 overflow-y-auto sm:max-w-sm">
		<Sheet.Header>
			<Sheet.Title class="leading-tight">{title}</Sheet.Title>
			<Sheet.Description class="text-xs">{object?.id ?? ''}</Sheet.Description>
		</Sheet.Header>
		{#if object}
			<div class="space-y-4 px-4">
				{#if object.properties && Object.keys(object.properties).length > 0}
					<div>
						<h4
							class="mb-2 text-xs font-medium text-muted-foreground uppercase tracking-wide"
						>
							Properties
						</h4>
						<div class="space-y-1.5">
							{#each Object.entries(object.properties) as [key, value] (key)}
								{#if value !== null && value !== undefined && value !== ''}
									<div
										class="flex items-baseline justify-between gap-2 text-sm"
									>
										<span class="text-muted-foreground capitalize"
											>{key.replace(/_/g, ' ')}</span
										>
										{#if String(value).startsWith('task_') && String(value).length > 5}
											<span class="font-mono text-xs opacity-60">ref</span>
										{:else}
											<span class="truncate font-medium">{String(value)}</span>
										{/if}
									</div>
								{/if}
							{/each}
						</div>
					</div>
					<Separator />
				{/if}

				<div>
					<h4
						class="mb-2 text-xs font-medium text-muted-foreground uppercase tracking-wide"
					>
						File & History
					</h4>
					<div class="space-y-2 text-sm">
						<div class="flex items-center justify-between">
							<span class="text-muted-foreground">Saved to</span>
							<span
								class="text-right text-xs font-mono opacity-70 truncate max-w-[160px]"
								>{object.relativePath}</span
							>
						</div>
						{#if object.revision}
							<div class="flex items-center justify-between">
								<span class="text-muted-foreground">Revision</span>
								<code class="rounded bg-muted px-1 text-xs font-mono opacity-70"
									>{String(object.revision).slice(0, 8)}</code
								>
							</div>
						{/if}
						{#if object.updated}
							<div class="flex items-center justify-between">
								<span class="text-muted-foreground">Updated</span>
								<span class="text-xs opacity-70"
									>{new Date(object.updated).toLocaleString()}</span
								>
							</div>
						{/if}
					</div>
				</div>

				{#if object.body}
					<Separator />
					<div>
						<h4
							class="mb-2 text-xs font-medium text-muted-foreground uppercase tracking-wide"
						>
							Description
						</h4>
						<p class="whitespace-pre-wrap text-sm leading-relaxed">
							{object.body}
						</p>
					</div>
				{/if}
			</div>
		{:else}
			<div
				class="flex flex-1 items-center justify-center p-8 text-center text-sm text-muted-foreground"
			>
				Select an item to view details
			</div>
		{/if}
	</Sheet.Content>
</Sheet.Root>
