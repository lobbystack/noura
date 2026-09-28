<script lang="ts">
	import { onMount } from 'svelte';
	import { workspace, getNouraClient } from '$lib/state.svelte';
	import * as Field from '$lib/components/ui/field';
	import * as Item from '$lib/components/ui/item';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import X from 'phosphor-svelte/lib/X';

	const client = getNouraClient();

	let ignore = $state<string[]>([]);
	let pattern = $state('');
	let savingIgnore = $state(false);
	let ignoreError = $state('');

	onMount(() => {
		if (!workspace.isReady) return;
		void client.manifest
			.read()
			.then((manifest) => (ignore = manifest.ignore))
			.catch(() => (ignoreError = 'Couldn’t read the ignore list.'));
	});

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
</script>

{#if workspace.isReady}
	<div class="flex flex-col gap-4">
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
				{#if ignoreError}<Field.FieldError>{ignoreError}</Field.FieldError>{/if}
			</Field.Field>
		</form>
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
