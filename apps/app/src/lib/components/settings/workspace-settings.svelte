<script lang="ts">
	import { workspace, diagnostics, getNouraClient } from '$lib/state.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Badge } from '$lib/components/ui/badge';
	import * as Empty from '$lib/components/ui/empty';
	let { section }: { section: 'general' | 'advanced' } = $props();
	let rebuilding = $state(false);
	let error = $state('');
	let notice = $state('');

	async function rebuildIndex() {
		rebuilding = true;
		error = '';
		notice = '';
		try {
			await getNouraClient().workspaces.rebuildIndex();
			await workspace.refresh();
			notice = 'Index rebuilt.';
		} catch (cause) {
			error =
				cause instanceof Error
					? cause.message
					: 'Could not rebuild the index. Try again.';
		} finally {
			rebuilding = false;
		}
	}
</script>

{#if workspace.isReady && workspace.state}
	<dl class="flex flex-col divide-y divide-border">
		{#if section === 'general'}
			<div class="flex flex-wrap items-center justify-between gap-4 py-4">
				<dt>Workspace name</dt>
				<dd>{workspace.name}</dd>
			</div>
			<div class="flex flex-col gap-2 py-4">
				<dt>Folder location</dt>
				<dd class="break-all text-sm text-muted-foreground select-text">
					{workspace.state.rootPath}
				</dd>
			</div>
		{:else}
			<div class="flex items-center justify-between gap-4 py-4">
				<dt>Indexed files</dt>
				<dd>
					<Badge variant="secondary">{workspace.state.indexedFiles}</Badge>
				</dd>
			</div>
			<div class="flex items-center justify-between gap-4 py-4">
				<dt>Detected issues</dt>
				<dd>
					<Badge
						variant={diagnostics.issues.length ? 'destructive' : 'secondary'}
						>{diagnostics.issues.length}</Badge
					>
				</dd>
			</div>
			<div class="flex flex-wrap items-center justify-between gap-4 py-5">
				<div class="flex min-w-0 flex-1 flex-col gap-1">
					<dt>Rebuild search index</dt>
					<dd class="text-sm leading-relaxed text-muted-foreground">
						Rebuild from workspace files without changing them.
					</dd>
				</div>
				<Button variant="outline" disabled={rebuilding} onclick={rebuildIndex}
					>{rebuilding ? 'Rebuilding…' : 'Rebuild index'}</Button
				>
			</div>
		{/if}
	</dl>
	{#if notice}<p role="status" class="text-sm text-muted-foreground">
			{notice}
		</p>{/if}
	{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
{:else}
	<Empty.Root
		><Empty.Header
			><Empty.Title>No workspace open</Empty.Title><Empty.Description
				>Open a workspace to view its settings.</Empty.Description
			></Empty.Header
		></Empty.Root
	>
{/if}
