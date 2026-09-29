<script lang="ts">
	import { workspace } from '$lib/state.svelte';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		problem = null,
	}: {
		/** Why the last workspace could not open. Shows a retry action. */
		problem?: string | null;
	} = $props();

	let creating = $state(false);
	let workspaceName = $state('');

	async function createWorkspace(event: SubmitEvent) {
		event.preventDefault();
		const name = workspaceName.trim();
		if (name) await workspace.pickAndCreate(name);
	}
</script>

<main class="flex min-h-0 flex-1 items-center justify-center px-6 py-12">
	<Empty.Root class="w-full max-w-lg">
		<Empty.Header>
			<Empty.Media variant="icon">
				{#if problem}<Warning />{:else}<FolderOpen />{/if}
			</Empty.Media>
			{#if problem}
				<Empty.Title>Can’t open the workspace</Empty.Title>
				<Empty.Description>{problem}</Empty.Description>
			{:else}
				<Empty.Title>Open a folder</Empty.Title>
				<Empty.Description>
					noura adds a small .noura folder. Your files stay as they are.
				</Empty.Description>
			{/if}
		</Empty.Header>

		<Empty.Content class="items-stretch">
			<div class="flex flex-col gap-2 sm:flex-row sm:justify-center">
				<Button
					disabled={workspace.isLoading}
					onclick={() => workspace.pickAndOpen()}
				>
					{#if workspace.isLoading}<Spinner
							data-icon="inline-start"
						/>{:else}<FolderOpen data-icon="inline-start" />{/if}
					Open folder…
				</Button>
				{#if problem}
					<Button
						variant="outline"
						disabled={workspace.isLoading}
						onclick={() => workspace.retry()}>Try again</Button
					>
				{/if}
			</div>

			{#if workspace.recents.length > 0}
				<Separator />
				<div class="flex flex-col gap-1">
					<p class="px-2 text-xs text-muted-foreground">Recent</p>
					{#each workspace.recents as recent (recent.workspaceId)}
						<Button
							variant="ghost"
							class="h-auto justify-start px-2 py-2 text-left"
							disabled={workspace.isLoading}
							onclick={() => workspace.open(recent.path)}
						>
							<span class="flex min-w-0 flex-col items-start">
								<span class="max-w-full truncate font-medium"
									>{recent.name}</span
								>
								<span
									class="max-w-full truncate text-xs font-normal text-muted-foreground"
									>{recent.path}</span
								>
							</span>
						</Button>
					{/each}
				</div>
			{/if}

			<Separator />
			{#if creating}
				<form class="flex flex-col gap-3" onsubmit={createWorkspace}>
					<Field.Group>
						<Field.Field>
							<Field.Label for="new-workspace-name">New workspace</Field.Label>
							<div class="flex gap-2">
								<Input
									id="new-workspace-name"
									bind:value={workspaceName}
									placeholder="Name"
									autocomplete="off"
									{@attach (node: HTMLInputElement) => node.focus()}
								/>
								<Button
									type="submit"
									variant="outline"
									disabled={workspace.isLoading ||
										workspaceName.trim().length === 0}
								>
									Choose folder…
								</Button>
							</div>
						</Field.Field>
					</Field.Group>
				</form>
			{:else}
				<Button
					variant="ghost"
					size="sm"
					class="self-center text-muted-foreground"
					onclick={() => (creating = true)}>New workspace…</Button
				>
			{/if}

			{#if workspace.error && !problem}
				<p class="text-sm text-destructive" role="alert">{workspace.error}</p>
			{/if}
		</Empty.Content>
	</Empty.Root>
</main>
