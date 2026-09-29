<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as Field from '$lib/components/ui/field';
	import DesktopRecoveryKit from './desktop-recovery-kit.svelte';
	import { errorText } from './copy';
	import { downloadJson, readJsonFile, recoveryKitFilename } from './files';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let exportPassphrase = $state('');
	let importPassphrase = $state('');
	let kitFile = $state<File | null>(null);
	let action = $state<'export' | 'import' | null>(null);
	let error = $state('');
	let notice = $state('');

	const canExport = $derived(
		panel.unlocked && !!exportPassphrase && action === null,
	);
	const canImport = $derived(
		!!panel.workspaceId && !!kitFile && !!importPassphrase && action === null,
	);

	function failureText(code: string, message: string): string {
		if (code === 'passphrase_rejected')
			return "That passphrase didn't open the kit.";
		return message || 'Something went wrong with the recovery kit.';
	}

	async function exportKit() {
		const controller = panel.controller;
		if (!controller || !canExport) return;
		action = 'export';
		error = '';
		notice = '';
		try {
			const result = await controller.exportRecoveryKit(exportPassphrase);
			if (!result.ok) {
				error = failureText(result.code, result.message);
				return;
			}
			downloadJson(
				result.value,
				recoveryKitFilename(panel.deviceId ?? panel.workspaceId),
			);
			exportPassphrase = '';
			notice = 'Recovery kit saved. Keep the file somewhere private.';
		} catch (cause) {
			error = errorText(cause, "The recovery kit couldn't be saved.");
		} finally {
			action = null;
		}
	}

	function chooseKit(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		kitFile = input.files?.[0] ?? null;
		// Clear the input so you can choose the same file again.
		input.value = '';
		error = '';
		notice = '';
	}

	async function importKit() {
		const controller = panel.controller;
		const workspaceId = panel.workspaceId;
		if (!controller || action !== null) return;
		if (!workspaceId) {
			error = 'Open a workspace to restore a kit.';
			return;
		}
		if (!kitFile) {
			error = 'Choose a recovery kit file.';
			return;
		}
		if (!importPassphrase) {
			error = 'Enter the kit passphrase.';
			return;
		}
		action = 'import';
		error = '';
		notice = '';
		try {
			const parsed = await readJsonFile(kitFile);
			if (parsed === undefined) {
				error = "That file isn't a recovery kit.";
				return;
			}
			const result = await controller.importRecoveryKit(
				parsed,
				importPassphrase,
				workspaceId,
			);
			if (!result.ok) {
				error = failureText(result.code, result.message);
				return;
			}
			importPassphrase = '';
			kitFile = null;
			notice =
				'Kit restored. Unlock this browser with the passphrase from the original browser.';
			await panel.refresh();
		} catch (cause) {
			error = errorText(cause, "The recovery kit couldn't be restored.");
		} finally {
			action = null;
		}
	}
</script>

<div
	class="flex flex-col gap-3"
	aria-labelledby="browser-sync-recovery-heading"
>
	<h4 id="browser-sync-recovery-heading" class="text-sm font-medium">
		Recovery kit
	</h4>
	<p class="text-xs text-muted-foreground">
		A recovery kit restores sync on another browser, so keep it somewhere
		private.
	</p>

	{#if panel.unlocked}
		<div class="flex flex-col gap-3 rounded-xl border p-4">
			<h5 class="text-xs font-medium">Save a recovery kit</h5>
			<Field.Field>
				<Field.Label for="browser-sync-kit-export-passphrase"
					>Kit passphrase</Field.Label
				>
				<Input
					id="browser-sync-kit-export-passphrase"
					type="password"
					autocomplete="new-password"
					bind:value={exportPassphrase}
					disabled={action !== null}
				/>
				<Field.Description
					>You'll need this passphrase to restore the kit.</Field.Description
				>
			</Field.Field>
			<div>
				<Button
					type="button"
					variant="outline"
					disabled={!canExport}
					onclick={exportKit}
				>
					{#if action === 'export'}
						<Spinner data-icon="inline-start" />Saving…
					{:else}
						Save recovery kit
					{/if}
				</Button>
			</div>
		</div>
	{/if}

	<div class="flex flex-col gap-3 rounded-xl border p-4">
		<h5 class="text-xs font-medium">Restore from a recovery kit</h5>
		<Field.Field>
			<Field.Label for="browser-sync-kit-file">Kit file</Field.Label>
			<Input
				id="browser-sync-kit-file"
				type="file"
				accept=".noura-recovery-kit.json,application/json"
				disabled={action !== null}
				onchange={chooseKit}
			/>
			<Field.Description>
				{#if kitFile}
					Selected {kitFile.name}.
				{:else}
					Choose a kit you saved from noura in a browser.
				{/if}
			</Field.Description>
		</Field.Field>
		<Field.Field>
			<Field.Label for="browser-sync-kit-import-passphrase"
				>Kit passphrase</Field.Label
			>
			<Input
				id="browser-sync-kit-import-passphrase"
				type="password"
				autocomplete="current-password"
				bind:value={importPassphrase}
				disabled={action !== null || !panel.workspaceId}
			/>
			<Field.Description>
				{#if panel.workspaceId}
					Use the passphrase you set when you saved the kit.
				{:else}
					Open a workspace to restore a kit.
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
				{#if action === 'import'}
					<Spinner data-icon="inline-start" />Restoring…
				{:else}
					Restore
				{/if}
			</Button>
		</div>
	</div>
	{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
	{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}

	<DesktopRecoveryKit {panel} />
</div>
