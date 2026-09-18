<script lang="ts">
	import { onMount } from 'svelte';
	import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
	import {
		getBrowserSyncController,
		MAX_BROWSER_ATTACHMENT_BYTES,
		type BrowserSyncController,
	} from '$lib/browser-sync';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';

	let {
		noteId,
		notePath,
		workspaceFiles = null,
	}: {
		noteId: string;
		notePath: string;
		workspaceFiles?: BrowserWorkspaceFiles | null;
	} = $props();

	let controller = $state<BrowserSyncController | null>(null);
	let items = $state<Array<{ path: string; name: string }>>([]);
	let busy = $state(false);
	let loading = $state(true);
	let error = $state('');
	let status = $state('');
	let progress = $state<{ uploaded: number; total: number } | null>(null);

	const limitLabel = `${Math.floor(
		MAX_BROWSER_ATTACHMENT_BYTES / (1024 * 1024),
	)} MB`;

	function percent(): number {
		if (!progress || progress.total === 0) return 0;
		return Math.min(
			100,
			Math.round((progress.uploaded / progress.total) * 100),
		);
	}

	async function load() {
		const value = controller;
		if (!value) return;
		loading = true;
		const result = await value.listNoteAttachments({ noteId, notePath });
		if (result.ok) {
			items = result.value;
			error = '';
		} else {
			items = [];
			error = result.message;
		}
		loading = false;
	}

	onMount(() => {
		let disposed = false;
		void (async () => {
			const value = await getBrowserSyncController();
			if (disposed) return;
			controller = value;
			if (workspaceFiles) value.setWorkspaceFiles(workspaceFiles);
			await load();
		})().catch((cause) => {
			if (!disposed) {
				error =
					cause instanceof Error
						? cause.message
						: 'Attachments are unavailable in this browser.';
				loading = false;
			}
		});
		return () => {
			disposed = true;
		};
	});

	async function attach(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		const value = controller;
		if (!file || !value || busy) return;
		if (file.size > MAX_BROWSER_ATTACHMENT_BYTES) {
			error = `This file is larger than the ${limitLabel} browser attachment limit.`;
			return;
		}
		busy = true;
		error = '';
		status = '';
		progress = { uploaded: 0, total: file.size };
		try {
			const bytes = new Uint8Array(await file.arrayBuffer());
			const send = () =>
				value.sendAttachment({
					noteId,
					notePath,
					name: file.name,
					bytes,
					onProgress: (uploaded, total) => {
						progress = { uploaded, total };
					},
				});
			let result = await send();
			// A note created since the last reconcile has no remote object yet.
			// Provision it through the normal sync pass, then retry once.
			if (!result.ok && result.code === 'object_not_found') {
				await value.syncNow();
				result = await send();
			}
			if (!result.ok) {
				error = result.message;
				return;
			}
			status = `Uploaded “${file.name}”. Publishing…`;
			const outcome = await value.syncNow();
			status =
				outcome.status === 'synced'
					? `“${file.name}” attached and published.`
					: `“${file.name}” uploaded and queued. ${outcome.message}`;
			await load();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'The attachment failed.';
		} finally {
			busy = false;
			progress = null;
		}
	}

	async function download(item: { path: string; name: string }) {
		const value = controller;
		if (!value || busy) return;
		error = '';
		const result = await value.readNoteAttachment({
			noteId,
			notePath,
			path: item.path,
		});
		if (!result.ok) {
			error = result.message;
			return;
		}
		const url = URL.createObjectURL(new Blob([result.value.bytes]));
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = result.value.name;
		anchor.rel = 'noopener';
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		setTimeout(() => URL.revokeObjectURL(url), 0);
	}
</script>

<section class="mt-4 flex flex-col gap-2" aria-label="Attachments">
	<h2 class="text-sm font-medium">Attachments</h2>
	<p class="text-sm text-muted-foreground">
		Files are encrypted with this note's object key and synchronized. Browser
		limit: {limitLabel} per file.
	</p>
	<Field.Field>
		<Field.Label for={`note-attachment-${noteId}`}>Attach a file</Field.Label>
		<Input
			id={`note-attachment-${noteId}`}
			type="file"
			disabled={busy || !controller}
			onchange={attach}
		/>
	</Field.Field>
	{#if progress}
		<p role="status" class="text-sm text-muted-foreground">
			Uploading… {percent()}% ({progress.uploaded} of {progress.total} bytes)
		</p>
	{/if}
	{#if loading}
		<p class="text-sm text-muted-foreground" role="status">
			Loading attachments…
		</p>
	{:else if items.length === 0}
		<p class="text-sm text-muted-foreground">No attachments for this note.</p>
	{:else}
		<ul class="flex flex-col gap-1">
			{#each items as item (item.path)}
				<li class="flex items-center justify-between gap-2">
					<span class="truncate text-sm">{item.name}</span>
					<Button
						type="button"
						variant="outline"
						size="sm"
						disabled={busy}
						onclick={() => void download(item)}
					>
						Download
					</Button>
				</li>
			{/each}
		</ul>
	{/if}
	{#if status}<p class="text-sm text-muted-foreground" role="status">
			{status}
		</p>{/if}
	{#if error}<p class="text-sm text-destructive" role="alert">{error}</p>{/if}
</section>
