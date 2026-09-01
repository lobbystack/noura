<script lang="ts">
	import type { Note, Project, Task, WorkspaceObject } from '@noura/workspace';
	import { getNouraClient as getClient } from '$lib/state.svelte';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { toast } from 'svelte-sonner';
	import Copy from 'phosphor-svelte/lib/Copy';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Terminal from 'phosphor-svelte/lib/Terminal';
	import FileCode from 'phosphor-svelte/lib/FileCode';

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
	const title = $derived(object?.title ?? 'Untitled');
	const source = $derived(
		object
			? `---\n${object.id ? `id: ${object.id}\n` : ''}type: ${object.type}\n---\n\n${object.body ?? ''}`
			: '',
	);

	async function copyPath() {
		if (!object?.relativePath) return;
		try {
			await navigator.clipboard.writeText(object.relativePath);
			toast.success('Copied relative path');
		} catch {
			toast.error('Could not copy path');
		}
	}

	async function revealInFolder() {
		if (!object?.id) return;
		try {
			await getClient().notes.showInFolder(object.id);
		} catch (error) {
			toast.error('Could not open the folder', {
				description: error instanceof Error ? error.message : String(error),
			});
		}
	}

	async function openTerminal() {
		if (!object?.id) return;
		try {
			await getClient().notes.openTerminal(object.id);
		} catch (error) {
			toast.error('Could not open a terminal', {
				description: error instanceof Error ? error.message : String(error),
			});
		}
	}
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
							class="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground"
						>
							Properties
						</h4>
						<div class="space-y-1.5">
							{#each Object.entries(object.properties) as [key, value] (key)}
								{#if value !== null && value !== undefined && value !== ''}
									<div
										class="flex items-baseline justify-between gap-2 text-sm"
									>
										<span class="capitalize text-muted-foreground"
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
					<div class="mb-2 flex items-center justify-between">
						<h4
							class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
						>
							File & history
						</h4>
						<Badge variant="outline" class="gap-1 text-[10px]">
							<FileCode data-icon="inline-start" />
							Markdown
						</Badge>
					</div>
					<div class="space-y-2 text-sm">
						<div class="flex items-center justify-between gap-2">
							<span class="text-muted-foreground">Saved to</span>
							<button
								class="max-w-40 truncate text-right font-mono text-xs opacity-70 hover:opacity-100"
								onclick={copyPath}
								title="Copy relative path"
							>
								{object.relativePath}
							</button>
						</div>
						{#if object.updated}
							<div class="flex items-center justify-between">
								<span class="text-muted-foreground">Updated</span>
								<span class="text-xs opacity-70"
									>{new Date(object.updated).toLocaleString()}</span
								>
							</div>
						{/if}
						{#if object.created}
							<div class="flex items-center justify-between">
								<span class="text-muted-foreground">Created</span>
								<span class="text-xs opacity-70"
									>{new Date(object.created).toLocaleString()}</span
								>
							</div>
						{/if}
						<div class="pt-1">
							<p class="mb-1.5 text-xs font-medium text-muted-foreground">
								Source
								<span class="ml-1 text-[10px] font-normal opacity-70"
									>read-only · canonical Markdown</span
								>
							</p>
							<pre
								class="max-h-40 overflow-auto rounded-md bg-muted p-2 font-mono text-[11px] leading-relaxed"><code
									>{source}</code
								></pre>
						</div>
						<div class="flex gap-2 pt-1.5">
							<Button
								variant="outline"
								size="sm"
								class="h-7 flex-1 text-xs"
								onclick={copyPath}
							>
								<Copy data-icon="inline-start" />
								Copy path
							</Button>
							<Button
								variant="outline"
								size="sm"
								class="h-7 flex-1 text-xs"
								onclick={revealInFolder}
							>
								<FolderOpen data-icon="inline-start" />
								Reveal
							</Button>
							<Button
								variant="outline"
								size="sm"
								class="h-7 flex-1 text-xs"
								onclick={openTerminal}
							>
								<Terminal data-icon="inline-start" />
								Terminal
							</Button>
						</div>
					</div>
				</div>
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
