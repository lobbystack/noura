<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as Field from '$lib/components/ui/field';
	import { extractEmbeddedRecoveryIdentity } from '$lib/browser-sync';
	import { errorText } from './copy';
	import { readJsonFile } from './files';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let kitFile = $state<File | null>(null);
	let recoveryKey = $state('');
	let keyInKit = $state(false);
	let busy = $state(false);
	let error = $state('');
	let notice = $state('');

	const canImport = $derived(
		panel.unlocked &&
			!!panel.workspaceId &&
			!!kitFile &&
			recoveryKey.trim().length > 0 &&
			!busy,
	);

	function clearKit() {
		kitFile = null;
		keyInKit = false;
		recoveryKey = '';
	}

	/**
	 * Fill in the recovery key when the kit includes one, and lock the field so
	 * nobody can swap in a different key.
	 */
	async function chooseKit(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0] ?? null;
		// Clear the input so you can choose the same file again.
		input.value = '';
		clearKit();
		error = '';
		notice = '';
		if (!file) return;
		const parsed = await readJsonFile(file);
		let embedded: string | null;
		try {
			if (parsed === undefined) throw new Error('Not a recovery kit');
			embedded = extractEmbeddedRecoveryIdentity(parsed);
		} catch {
			error = "That file isn't a desktop recovery kit.";
			return;
		}
		kitFile = file;
		keyInKit = embedded !== null;
		recoveryKey = embedded ?? '';
	}

	async function importKit() {
		const controller = panel.controller;
		const workspaceId = panel.workspaceId;
		if (!controller || busy) return;
		if (!workspaceId) {
			error = 'Open a workspace to restore a kit.';
			return;
		}
		if (!kitFile) {
			error = 'Choose a desktop kit file.';
			return;
		}
		const recoveryIdentity = recoveryKey.trim();
		if (!recoveryIdentity) {
			error = 'Paste the recovery key from the kit.';
			return;
		}
		busy = true;
		error = '';
		notice = '';
		try {
			const parsed = await readJsonFile(kitFile);
			if (parsed === undefined) {
				error = "That file isn't a desktop recovery kit.";
				return;
			}
			const result = await controller.importNativeRecoveryKit(parsed, {
				recoveryIdentity,
				localWorkspaceId: workspaceId,
			});
			if (!result.ok) {
				error = result.message || "The kit couldn't be restored.";
				return;
			}
			// Drop the recovery key from the form as soon as it has been used.
			clearKit();
			notice = 'Restored your workspace keys to this browser.';
			await panel.refresh();
		} catch (cause) {
			error = errorText(cause, "The kit couldn't be restored.");
		} finally {
			busy = false;
		}
	}
</script>

<div class="flex flex-col gap-3 rounded-xl border p-4">
	<h5 class="text-xs font-medium">Restore from a desktop app kit</h5>
	<p class="text-xs text-muted-foreground">
		Use a recovery kit from the noura desktop app to give this browser your
		workspace keys.
	</p>
	<Field.Field>
		<Field.Label for="browser-sync-desktop-kit-file"
			>Desktop kit file</Field.Label
		>
		<Input
			id="browser-sync-desktop-kit-file"
			type="file"
			accept=".json,application/json"
			disabled={busy}
			onchange={chooseKit}
		/>
		<Field.Description>
			{#if kitFile}
				Selected {kitFile.name}.
			{:else}
				Choose the kit you saved in the noura desktop app.
			{/if}
		</Field.Description>
	</Field.Field>
	<Field.Field>
		<Field.Label for="browser-sync-desktop-recovery-key"
			>Recovery key</Field.Label
		>
		<Input
			id="browser-sync-desktop-recovery-key"
			type="password"
			autocomplete="off"
			bind:value={recoveryKey}
			disabled={busy || keyInKit || !panel.workspaceId}
		/>
		<Field.Description>
			{#if keyInKit}
				This kit includes its recovery key, so noura filled it in.
			{:else if !panel.workspaceId}
				Open a workspace to restore a kit.
			{:else}
				Paste the recovery key from your kit; noura never stores it.
			{/if}
		</Field.Description>
	</Field.Field>
	<div>
		<Button
			type="button"
			variant="outline"
			disabled={!canImport}
			onclick={importKit}
		>
			{#if busy}
				<Spinner data-icon="inline-start" />Restoring…
			{:else}
				Restore
			{/if}
		</Button>
	</div>
	{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
	{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
</div>
