<script lang="ts">
	import { onMount } from 'svelte';
	import { isCoreError, type TrashEntry } from '@noura/workspace';
	import { workspace, getNouraClient } from '$lib/state.svelte';
	import * as Field from '$lib/components/ui/field';
	import * as Item from '$lib/components/ui/item';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Badge } from '$lib/components/ui/badge';
	import X from 'phosphor-svelte/lib/X';
	import Trash from 'phosphor-svelte/lib/Trash';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';

	const client = getNouraClient();

	let ignore = $state<string[]>([]);
	let pattern = $state('');
	let savingIgnore = $state(false);
	let ignoreError = $state('');

	let trash = $state.raw<TrashEntry[] | null>(null);
	let restoring = $state<string | null>(null);
	let trashError = $state('');
	let trashNotice = $state('');

	const dateFormat = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short',
	});
	const sizeFormat = new Intl.NumberFormat(undefined, {
		style: 'unit',
		unit: 'kilobyte',
		maximumFractionDigits: 1,
	});

	onMount(() => {
		if (!workspace.isReady) return;
		void client.manifest
			.read()
			.then((manifest) => (ignore = manifest.ignore))
			.catch(() => (ignoreError = 'Couldn’t read the ignore list.'));
		void loadTrash();
	});

	async function loadTrash() {
		try {
			trash = await client.trash.list();
		} catch {
			trashError = 'Couldn’t read the trash.';
		}
	}

	async function saveIgnore(next: string[]) {
		savingIgnore = true;
		ignoreError = '';
		try {
			const manifest = await client.manifest.read();
			const saved = await client.manifest.update({
				ignore: next,
				expectedUpdated: manifest.updated,
			});
			ignore = saved.ignore;
			pattern = '';
			await workspace.refresh();
		} catch {
			ignoreError =
				'Couldn’t save the ignore list. Its settings file may have changed on disk. Try again.';
		} finally {
			savingIgnore = false;
		}
	}

	function addPattern(event: SubmitEvent) {
		event.preventDefault();
		const value = pattern.trim();
		if (!value || ignore.includes(value)) {
			pattern = '';
			return;
		}
		void saveIgnore([...ignore, value]);
	}

	async function restore(entry: TrashEntry) {
		restoring = entry.trashPath;
		trashError = '';
		trashNotice = '';
		try {
			await client.trash.restore(entry.trashPath);
			trashNotice = `Restored ${entry.originalPath}.`;
			await Promise.all([loadTrash(), workspace.refresh()]);
		} catch (cause) {
			trashError =
				isCoreError(cause) && cause.code === 'trash_restore_target_exists'
					? `A file already exists at ${entry.originalPath}. Move or rename it, then restore again.`
					: 'Couldn’t restore this item.';
		} finally {
			restoring = null;
		}
	}
</script>

{#if workspace.isReady}
	<div class="flex flex-col gap-10">
		<section class="flex flex-col gap-4" aria-labelledby="ignore-heading">
			<div class="flex flex-col gap-1">
				<h3 id="ignore-heading" class="text-sm font-medium">Ignored files</h3>
				<p class="text-sm text-muted-foreground">
					noura won’t index or show files that match these patterns. They use
					the same rules as <code>.gitignore</code>, for example
					<code>drafts/</code> or <code>*.tmp</code>.
				</p>
			</div>
			{#if ignore.length > 0}
				<Item.Group>
					{#each ignore as value (value)}
						<Item.Root variant="outline" size="sm">
							<Item.Content>
								<Item.Title><code>{value}</code></Item.Title>
							</Item.Content>
							<Item.Actions>
								<Button
									variant="ghost"
									size="icon-sm"
									aria-label={`Stop ignoring ${value}`}
									disabled={savingIgnore}
									onclick={() =>
										saveIgnore(ignore.filter((item) => item !== value))}
								>
									<X />
								</Button>
							</Item.Actions>
						</Item.Root>
					{/each}
				</Item.Group>
			{/if}
			<form onsubmit={addPattern}>
				<Field.Field data-invalid={ignoreError ? true : undefined}>
					<Field.FieldLabel for="ignore-pattern" class="sr-only"
						>Pattern to ignore</Field.FieldLabel
					>
					<div class="flex gap-2">
						<Input
							id="ignore-pattern"
							placeholder="drafts/"
							autocomplete="off"
							bind:value={pattern}
							disabled={savingIgnore}
							aria-invalid={ignoreError ? true : undefined}
						/>
						<Button
							type="submit"
							variant="outline"
							disabled={savingIgnore || !pattern.trim()}
						>
							{#if savingIgnore}<Spinner data-icon="inline-start" />{/if}
							Add
						</Button>
					</div>
					{#if ignoreError}<Field.FieldError>{ignoreError}</Field.FieldError
						>{/if}
				</Field.Field>
			</form>
		</section>

		<section class="flex flex-col gap-4" aria-labelledby="trash-heading">
			<div class="flex flex-wrap items-start justify-between gap-3">
				<div class="flex flex-col gap-1">
					<h3 id="trash-heading" class="text-sm font-medium">Trash</h3>
					<p class="text-sm text-muted-foreground">
						Deleted notes, tasks, and expired chats wait here until you delete
						them from the trash folder.
					</p>
				</div>
				<Button
					variant="outline"
					size="sm"
					onclick={() => client.workspaces.showInFolder('trash')}
				>
					<FolderOpen data-icon="inline-start" />
					Show folder
				</Button>
			</div>
			{#if trash === null && !trashError}
				<p role="status" class="text-sm text-muted-foreground">
					Reading the trash…
				</p>
			{:else if trash && trash.length === 0}
				<Empty.Root class="border">
					<Empty.Header>
						<Empty.Media variant="icon"><Trash /></Empty.Media>
						<Empty.Title>The trash is empty</Empty.Title>
					</Empty.Header>
				</Empty.Root>
			{:else if trash}
				<Item.Group>
					{#each trash as entry (entry.trashPath)}
						<Item.Root variant="outline" size="sm">
							<Item.Content>
								<Item.Title class="break-all">
									{entry.originalPath}
									{#if entry.kind === 'chat'}<Badge variant="secondary"
											>Chat</Badge
										>{/if}
								</Item.Title>
								<Item.Description>
									{entry.deletedAt
										? dateFormat.format(new Date(entry.deletedAt))
										: 'Unknown date'} · {sizeFormat.format(entry.size / 1000)}
								</Item.Description>
							</Item.Content>
							<Item.Actions>
								<Button
									variant="outline"
									size="sm"
									disabled={restoring !== null}
									onclick={() => restore(entry)}
								>
									{#if restoring === entry.trashPath}<Spinner
											data-icon="inline-start"
										/>{/if}
									Restore
								</Button>
							</Item.Actions>
						</Item.Root>
					{/each}
				</Item.Group>
			{/if}
			{#if trashNotice}<p role="status" class="text-sm">{trashNotice}</p>{/if}
			{#if trashError}<p role="alert" class="text-sm text-destructive">
					{trashError}
				</p>{/if}
		</section>
	</div>
{:else}
	<Empty.Root>
		<Empty.Header>
			<Empty.Title>No workspace open</Empty.Title>
			<Empty.Description
				>Open a workspace to manage its files.</Empty.Description
			>
		</Empty.Header>
	</Empty.Root>
{/if}
