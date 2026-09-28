<script lang="ts">
	import { getAppUpdates } from '$lib/app-updates.svelte';
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';

	const updates = getAppUpdates();
	const state = $derived(updates.state);
	const busy = $derived(
		state.status === 'checking' ||
			state.status === 'downloading' ||
			state.status === 'installing',
	);
	const timeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: 'short' });
</script>

<Field.Field>
	<Field.FieldLabel>Updates</Field.FieldLabel>
	<Field.FieldDescription>
		{#if updates.supported}
			You have version {updates.version ?? '…'}. noura downloads new versions in
			the background, and you choose when to restart.
		{:else}
			You have version {updates.version ?? '…'}. This build doesn’t update
			itself; install noura from noura.app to get updates.
		{/if}
	</Field.FieldDescription>

	{#if updates.supported}
		{#if state.status === 'ready'}
			<div class="flex flex-col gap-3 rounded-lg border p-4">
				<p class="text-sm font-medium">Version {state.version} is ready</p>
				{#if state.notes}
					<p class="text-sm whitespace-pre-line text-muted-foreground">
						{state.notes}
					</p>
				{/if}
				{#if state.error}<p role="alert" class="text-sm text-destructive">
						{state.error}
					</p>{/if}
				<div>
					<Button onclick={() => updates.installAndRestart()}
						>Restart to update</Button
					>
				</div>
			</div>
		{:else}
			<div class="flex flex-wrap items-center gap-3">
				<Button
					variant="outline"
					disabled={busy}
					onclick={() => updates.check()}
				>
					{#if busy}<Spinner data-icon="inline-start" />{/if}
					Check for updates
				</Button>
				<p role="status" class="text-sm text-muted-foreground">
					{#if state.status === 'downloading'}
						Downloading version {state.version}…
					{:else if state.status === 'installing'}
						Installing version {state.version}…
					{:else if state.status === 'idle' && state.lastCheck?.outcome === 'current'}
						Up to date as of {timeFormat.format(state.lastCheck.at)}.
					{:else if state.status === 'idle' && state.lastCheck?.outcome === 'failed'}
						<span class="text-destructive"
							>Couldn’t reach the update server. Check your connection.</span
						>
					{/if}
				</p>
			</div>
		{/if}
	{/if}
</Field.Field>
