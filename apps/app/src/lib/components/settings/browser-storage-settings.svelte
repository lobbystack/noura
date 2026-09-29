<script lang="ts">
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import {
		downloadWorkspaceBackup,
		requestPersistentStorage,
		restoreWorkspaceBackup,
		storageIsPersistent,
	} from '$lib/browser-backup-actions';
	import { workspace } from '$lib/state.svelte';
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import DownloadSimple from 'phosphor-svelte/lib/DownloadSimple';
	import UploadSimple from 'phosphor-svelte/lib/UploadSimple';

	let persistent = $state<boolean | null>(null);
	let busy = $state<'backup' | 'restore' | null>(null);
	let error = $state('');
	let fileInput: HTMLInputElement | undefined;

	onMount(() => {
		void storageIsPersistent().then((value) => (persistent = value));
	});

	function message(cause: unknown) {
		return cause instanceof Error ? cause.message : 'Something went wrong.';
	}

	async function keepFiles() {
		persistent = await requestPersistentStorage();
		if (!persistent) toast('The browser didn’t agree to keep your files.');
	}

	async function backup() {
		busy = 'backup';
		error = '';
		try {
			await downloadWorkspaceBackup();
		} catch (cause) {
			error = message(cause);
		} finally {
			busy = null;
		}
	}

	async function restore(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		busy = 'restore';
		error = '';
		try {
			await restoreWorkspaceBackup(file);
			toast.success(`Opened “${workspace.name}”.`);
		} catch (cause) {
			error = message(cause);
		} finally {
			busy = null;
		}
	}
</script>

<Field.FieldGroup>
	<Field.Field>
		<Field.FieldLabel>This browser</Field.FieldLabel>
		<Field.FieldDescription>
			Your workspaces live in this browser on this device. Clearing site data
			deletes them.
		</Field.FieldDescription>
		{#if persistent === false}
			<div>
				<Button variant="outline" onclick={keepFiles}>Keep my files</Button>
			</div>
		{:else if persistent}
			<p class="text-sm text-muted-foreground">
				The browser keeps your files when the disk runs low.
			</p>
		{/if}
	</Field.Field>

	<Field.Field>
		<Field.FieldLabel>Back up</Field.FieldLabel>
		<Field.FieldDescription>
			Download a copy of this workspace.
		</Field.FieldDescription>
		<div>
			<Button
				variant="outline"
				disabled={busy !== null || !workspace.isReady}
				onclick={backup}
			>
				{#if busy === 'backup'}<Spinner
						data-icon="inline-start"
					/>{:else}<DownloadSimple data-icon="inline-start" />{/if}
				Download backup
			</Button>
		</div>
	</Field.Field>

	<Field.Field>
		<Field.FieldLabel for="restore-backup">Restore</Field.FieldLabel>
		<Field.FieldDescription>
			Open a backup as a new workspace in this browser.
		</Field.FieldDescription>
		<input
			{@attach (node: HTMLInputElement) => {
				fileInput = node;
			}}
			id="restore-backup"
			type="file"
			accept=".json,application/json"
			class="sr-only"
			onchange={restore}
		/>
		<div>
			<Button
				variant="outline"
				disabled={busy !== null}
				onclick={() => fileInput?.click()}
			>
				{#if busy === 'restore'}<Spinner
						data-icon="inline-start"
					/>{:else}<UploadSimple data-icon="inline-start" />{/if}
				Restore backup…
			</Button>
		</div>
	</Field.Field>

	{#if error}<Field.FieldError>{error}</Field.FieldError>{/if}
</Field.FieldGroup>
