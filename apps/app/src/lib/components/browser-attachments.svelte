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
	import { Progress } from '$lib/components/ui/progress';
	import * as Field from '$lib/components/ui/field';
	import { errorText } from './browser-sync/copy';

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

	const percent = $derived(
		!progress || progress.total === 0
			? 0
			: Math.min(100, Math.round((progress.uploaded / progress.total) * 100)),
	);

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
				error = errorText(
					cause,
					"Attachments aren't available in this browser.",
				);
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
			error = `This file is over the ${limitLabel} limit.`;
			return;
		}
		busy = true;
		error = '';
		status = '';
		progress = { uploaded: 0, total: file.size };
		try {
			const result = await value.attachAndSync({
				noteId,
				notePath,
				name: file.name,
				bytes: new Uint8Array(await file.arrayBuffer()),
				onProgress: (uploaded, total) => {
					progress = { uploaded, total };
				},
				onUploaded: () => {
					status = `Uploaded “${file.name}”. Syncing…`;
				},
			});
			if (!result.ok) {
				status = '';
				error = result.message;
				return;
			}
			const sync = result.value.sync;
			status =
				sync.status === 'synced'
					? `Attached “${file.name}”.`
					: `Uploaded “${file.name}”, but sync didn't finish. ${sync.message}`;
			await load();
		} catch (cause) {
			status = '';
			error = errorText(cause, 'The upload failed.');
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
		noura encrypts each file before upload, up to {limitLabel}.
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
		<div class="flex flex-col gap-1">
			<Progress value={percent} aria-label="Upload progress" />
			<p role="status" class="text-sm text-muted-foreground">
				Uploading… {percent}%
			</p>
		</div>
	{/if}
	{#if loading}
		<p class="text-sm text-muted-foreground" role="status">
			Loading attachments…
		</p>
	{:else if items.length === 0}
		<p class="text-sm text-muted-foreground">No attachments yet.</p>
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
	{#if status}
		<p class="text-sm text-muted-foreground" role="status">{status}</p>
	{/if}
	{#if error}<p class="text-sm text-destructive" role="alert">{error}</p>{/if}
</section>
