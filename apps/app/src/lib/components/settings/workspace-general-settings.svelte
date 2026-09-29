<script lang="ts">
	import { workspace, getNouraClient } from '$lib/state.svelte';
	import { hostCapabilities } from '$lib/host-capabilities.svelte';
	import * as Field from '$lib/components/ui/field';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';

	const client = getNouraClient();
	const folders = $derived(hostCapabilities.current.workspaceFolders);

	let draft = $derived(workspace.name);
	let saving = $state(false);
	let error = $state('');

	const trimmed = $derived(draft.trim());
	const changed = $derived(trimmed !== '' && trimmed !== workspace.name);

	async function rename(event: SubmitEvent) {
		event.preventDefault();
		if (!changed) return;
		saving = true;
		error = '';
		try {
			const manifest = await client.manifest.read();
			await client.manifest.update({
				name: trimmed,
				expectedUpdated: manifest.updated,
			});
			await workspace.refreshRecents();
		} catch {
			error = folders
				? 'Couldn’t rename the workspace. Its settings file may have changed on disk. Try again.'
				: 'Couldn’t rename the workspace. Try again.';
		} finally {
			saving = false;
		}
	}

	async function showFolder() {
		error = '';
		try {
			await client.workspaces.showInFolder('root');
		} catch {
			error = 'Couldn’t open the folder.';
		}
	}
</script>

{#if workspace.isReady && workspace.state?.rootPath}
	<form onsubmit={rename}>
		<Field.FieldGroup>
			<Field.Field data-invalid={error ? true : undefined}>
				<Field.FieldLabel for="workspace-name">Name</Field.FieldLabel>
				<div class="flex gap-2">
					<Input
						id="workspace-name"
						bind:value={draft}
						autocomplete="off"
						maxlength={120}
						aria-invalid={error ? true : undefined}
						disabled={saving}
					/>
					<Button type="submit" variant="outline" disabled={!changed || saving}>
						{#if saving}<Spinner data-icon="inline-start" />{/if}
						Rename
					</Button>
				</div>
				<Field.FieldDescription>
					{folders
						? 'Shown in the workspace switcher. The folder keeps its name.'
						: 'Shown in the workspace switcher.'}
				</Field.FieldDescription>
			</Field.Field>

			{#if folders}
				<Field.Field>
					<Field.FieldLabel>Location</Field.FieldLabel>
					<p class="break-all text-sm text-muted-foreground select-text">
						{workspace.state.rootPath}
					</p>
					<div>
						<Button type="button" variant="outline" onclick={showFolder}>
							<FolderOpen data-icon="inline-start" />
							Show folder
						</Button>
					</div>
				</Field.Field>
			{/if}
			{#if error}<Field.FieldError>{error}</Field.FieldError>{/if}
		</Field.FieldGroup>
	</form>
{:else}
	<Empty.Root>
		<Empty.Header>
			<Empty.Title>No workspace open</Empty.Title>
			<Empty.Description
				>Open a workspace to change its settings.</Empty.Description
			>
		</Empty.Header>
	</Empty.Root>
{/if}
