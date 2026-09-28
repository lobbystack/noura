<script lang="ts">
	import type { Diagnostic } from '@noura/workspace';
	import { workspace, diagnostics, getNouraClient } from '$lib/state.svelte';
	import * as Item from '$lib/components/ui/item';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import CheckCircle from 'phosphor-svelte/lib/CheckCircle';

	const client = getNouraClient();

	let rebuilding = $state(false);
	let error = $state('');
	let notice = $state('');

	const titles: Record<string, string> = {
		parse_error: 'noura can’t read this file',
		identity_conflict: 'Two files share the same ID',
		broken_reference: 'A link points to something that doesn’t exist',
		index_repair_pending: 'The search index needs a rebuild',
	};
	const fixes: Record<string, string> = {
		parse_error: 'Fix the frontmatter at the top of the file.',
		identity_conflict:
			'This usually happens when a file is copied. Delete one of the copies.',
		broken_reference: 'Update or remove the link.',
		index_repair_pending: 'Rebuild the index below.',
	};

	function title(diagnostic: Diagnostic) {
		return titles[diagnostic.code] ?? diagnostic.message;
	}

	async function rebuildIndex() {
		rebuilding = true;
		error = '';
		notice = '';
		try {
			await client.workspaces.rebuildIndex();
			await workspace.refresh();
			notice = 'Search index rebuilt.';
		} catch {
			error = 'Couldn’t rebuild the index. Try again.';
		} finally {
			rebuilding = false;
		}
	}
</script>

{#if workspace.isReady && workspace.state}
	<div class="flex flex-col gap-10">
		<section class="flex flex-col gap-4" aria-labelledby="issues-heading">
			<h3 id="issues-heading" class="text-sm font-medium">Problems</h3>
			{#if diagnostics.issues.length === 0}
				<Empty.Root class="border">
					<Empty.Header>
						<Empty.Media variant="icon"><CheckCircle /></Empty.Media>
						<Empty.Title>No problems found</Empty.Title>
						<Empty.Description>
							noura read every file in this workspace.
						</Empty.Description>
					</Empty.Header>
				</Empty.Root>
			{:else}
				<Item.Group>
					{#each diagnostics.issues as issue, index (`${issue.code}:${issue.relativePath ?? ''}:${issue.objectId ?? ''}:${index}`)}
						<Item.Root variant="outline" size="sm">
							<Item.Content>
								<Item.Title>{title(issue)}</Item.Title>
								<Item.Description class="break-all">
									{#if issue.relativePath}{issue.relativePath} ·
									{/if}{fixes[issue.code] ?? issue.message}
								</Item.Description>
							</Item.Content>
							{#if issue.objectId}
								<Item.Actions>
									<Button
										variant="outline"
										size="sm"
										onclick={() =>
											issue.objectId &&
											client.objects.showInFolder(issue.objectId)}
									>
										Show file
									</Button>
								</Item.Actions>
							{/if}
						</Item.Root>
					{/each}
				</Item.Group>
			{/if}
		</section>

		<section class="flex flex-col gap-3" aria-labelledby="index-heading">
			<div class="flex flex-col gap-1">
				<h3 id="index-heading" class="text-sm font-medium">Search index</h3>
				<p class="text-sm text-muted-foreground">
					{workspace.state.indexedFiles} files indexed. Rebuild if search misses files
					you know are there. Your files don’t change.
				</p>
			</div>
			<div>
				<Button variant="outline" disabled={rebuilding} onclick={rebuildIndex}>
					{#if rebuilding}<Spinner data-icon="inline-start" />{/if}
					Rebuild index
				</Button>
			</div>
			{#if notice}<p role="status" class="text-sm text-muted-foreground">
					{notice}
				</p>{/if}
			{#if error}<p role="alert" class="text-sm text-destructive">
					{error}
				</p>{/if}
		</section>
	</div>
{:else}
	<Empty.Root>
		<Empty.Header>
			<Empty.Title>No workspace open</Empty.Title>
			<Empty.Description
				>Open a workspace to check it for problems.</Empty.Description
			>
		</Empty.Header>
	</Empty.Root>
{/if}
