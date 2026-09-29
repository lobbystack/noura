<script lang="ts">
	import { workspace } from '$lib/state.svelte';
	import { hostCapabilities } from '$lib/host-capabilities.svelte';
	import { restoreWorkspaceBackup } from '$lib/browser-backup-actions';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import UploadSimple from 'phosphor-svelte/lib/UploadSimple';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		problem = null,
	}: {
		/** Why the last workspace could not open. Shows a retry action. */
		problem?: string | null;
	} = $props();

	// Desktop workspaces are folders; browser workspaces live in the browser.
	const folders = $derived(hostCapabilities.current.workspaceFolders);
	let creating = $state(false);
	let workspaceName = $state('');
	let restoreError = $state('');
	let fileInput: HTMLInputElement | undefined;

	async function createWorkspace(event: SubmitEvent) {
		event.preventDefault();
		const name = workspaceName.trim();
		if (name) await workspace.createNamed(name);
	}

	async function restore(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		restoreError = '';
		try {
			await restoreWorkspaceBackup(file);
		} catch (cause) {
			restoreError =
				cause instanceof Error ? cause.message : 'Couldn’t restore the backup.';
		}
	}
</script>

{#snippet createForm()}
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
						variant={folders ? 'outline' : 'default'}
						disabled={workspace.isLoading || workspaceName.trim().length === 0}
					>
						{folders ? 'Choose folder…' : 'Create'}
					</Button>
				</div>
			</Field.Field>
		</Field.Group>
	</form>
{/snippet}

<main class="flex min-h-0 flex-1 items-center justify-center px-6 py-12">
	<Empty.Root class="w-full max-w-lg">
		<Empty.Header>
			<Empty.Media variant="icon">
				{#if problem}<Warning />{:else}<FolderOpen />{/if}
			</Empty.Media>
			{#if problem}
				<Empty.Title>Can’t open the workspace</Empty.Title>
				<Empty.Description>{problem}</Empty.Description>
			{:else if folders}
				<Empty.Title>Open a folder</Empty.Title>
				<Empty.Description>
					noura adds a small .noura folder. Your files stay as they are.
				</Empty.Description>
			{:else}
				<Empty.Title>Create a workspace</Empty.Title>
				<Empty.Description>
					Your workspaces live in this browser on this device.
				</Empty.Description>
			{/if}
		</Empty.Header>

		<Empty.Content class="items-stretch">
			{#if folders || problem}
				<div class="flex flex-col gap-2 sm:flex-row sm:justify-center">
					{#if folders}
						<Button
							disabled={workspace.isLoading}
							onclick={() => workspace.pickAndOpen()}
						>
							{#if workspace.isLoading}<Spinner
									data-icon="inline-start"
								/>{:else}<FolderOpen data-icon="inline-start" />{/if}
							Open folder…
						</Button>
					{/if}
					{#if problem}
						<Button
							variant="outline"
							disabled={workspace.isLoading}
							onclick={() => workspace.retry()}>Try again</Button
						>
					{/if}
				</div>
			{/if}

			{#if !folders}
				{@render createForm()}
			{/if}

			{#if workspace.recents.length > 0}
				<Separator />
				<div class="flex flex-col gap-1">
					<p class="px-2 text-xs text-muted-foreground">
						{folders ? 'Recent' : 'Your workspaces'}
					</p>
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
								{#if folders}
									<span
										class="max-w-full truncate text-xs font-normal text-muted-foreground"
										>{recent.path}</span
									>
								{/if}
							</span>
						</Button>
					{/each}
				</div>
			{/if}

			<Separator />
			{#if !folders}
				<input
					{@attach (node: HTMLInputElement) => {
						fileInput = node;
					}}
					type="file"
					accept=".json,application/json"
					class="sr-only"
					aria-label="Backup file"
					onchange={restore}
				/>
				<Button
					variant="ghost"
					size="sm"
					class="self-center text-muted-foreground"
					disabled={workspace.isLoading}
					onclick={() => fileInput?.click()}
				>
					<UploadSimple data-icon="inline-start" />
					Restore backup…
				</Button>
				{#if restoreError}
					<p class="text-sm text-destructive" role="alert">{restoreError}</p>
				{/if}
			{:else if creating}
				{@render createForm()}
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
