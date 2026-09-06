<script lang="ts">
	import { workspace } from '$lib/state.svelte';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';

	let workspaceName = $state('');

	async function createWorkspace(event: SubmitEvent) {
		event.preventDefault();
		const name = workspaceName.trim();
		if (name) await workspace.pickAndCreate(name);
	}
</script>

<main class="flex min-h-0 flex-1 items-center justify-center px-6 py-12">
	<Empty.Root class="w-full max-w-2xl">
		<Empty.Header>
			<Empty.Media variant="icon">
				<FolderOpen />
			</Empty.Media>
			<Empty.Title>Open your workspace</Empty.Title>
			<Empty.Description>
				Noura works from an ordinary folder. Your Markdown files remain the
				durable source of truth.
			</Empty.Description>
		</Empty.Header>

		<Empty.Content class="max-w-lg items-stretch">
			{#if workspace.recents.length > 0}
				<div class="flex flex-col gap-1">
					<p class="px-1 text-xs font-medium text-muted-foreground">
						Recent workspaces
					</p>
					{#each workspace.recents as recent (recent.workspaceId)}
						<Button
							variant="ghost"
							class="h-auto justify-start px-2 py-2 text-left"
							disabled={workspace.isLoading}
							onclick={() => workspace.open(recent.path)}
						>
							<span class="flex min-w-0 flex-col items-start">
								<span class="truncate font-medium">{recent.name}</span>
								<span
									class="max-w-full truncate text-xs font-normal text-muted-foreground"
									>{recent.path}</span
								>
							</span>
						</Button>
					{/each}
				</div>
				<Separator />
			{/if}

			<div class="flex flex-col gap-2">
				<p class="text-sm text-muted-foreground">
					Choose an existing folder that contains a Noura workspace.
				</p>
				<Button
					type="button"
					disabled={workspace.isLoading}
					onclick={() => workspace.pickAndOpen()}
				>
					{#if workspace.isLoading}
						<Spinner data-icon="inline-start" />
					{:else}
						<FolderOpen data-icon="inline-start" />
					{/if}
					Open workspace
				</Button>
			</div>

			<Separator />

			<form class="flex flex-col gap-3" onsubmit={createWorkspace}>
				<Field.Group>
					<Field.Field>
						<Field.Label for="new-workspace-name">Workspace name</Field.Label>
						<Input
							id="new-workspace-name"
							bind:value={workspaceName}
							placeholder="My workspace"
							autocomplete="off"
						/>
					</Field.Field>
				</Field.Group>
				<p class="text-sm text-muted-foreground">
					You’ll choose or create its folder in Finder next.
				</p>
				<Button
					type="submit"
					variant="outline"
					disabled={workspace.isLoading || workspaceName.trim().length === 0}
				>
					{#if workspace.isLoading}
						<Spinner data-icon="inline-start" />
					{:else}
						<FolderOpen data-icon="inline-start" />
					{/if}
					Choose folder and create
				</Button>
			</form>

			{#if workspace.error}
				<p class="text-sm text-destructive" role="alert">{workspace.error}</p>
			{/if}
		</Empty.Content>
	</Empty.Root>
</main>
