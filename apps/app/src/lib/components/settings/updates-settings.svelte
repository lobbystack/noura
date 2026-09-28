<script lang="ts">
	import { getAppUpdates } from '$lib/app-updates.svelte';
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

<div class="flex flex-col gap-6">
	<dl class="flex flex-col divide-y divide-border">
		<div class="flex justify-between gap-4 py-3">
			<dt class="text-sm">Installed version</dt>
			<dd class="text-sm text-muted-foreground">{updates.version ?? '—'}</dd>
		</div>
	</dl>

	{#if !updates.supported}
		<p class="text-sm text-muted-foreground">
			This build doesn’t update itself. Install noura from noura.app to get
			updates automatically.
		</p>
	{:else}
		<p class="text-sm text-muted-foreground">
			noura checks for updates when it opens and every 6 hours, then downloads
			them in the background. You choose when to restart.
		</p>

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
					{#if state.status === 'checking'}
						Checking…
					{:else if state.status === 'downloading'}
						Downloading version {state.version}…
					{:else if state.status === 'installing'}
						Installing version {state.version}…
					{:else if state.lastCheck?.outcome === 'current'}
						You have the latest version. Checked at {timeFormat.format(
							state.lastCheck.at,
						)}.
					{:else if state.lastCheck?.outcome === 'failed'}
						<span class="text-destructive"
							>Couldn’t reach the update server. Check your connection.</span
						>
					{/if}
				</p>
			</div>
		{/if}
	{/if}
</div>
