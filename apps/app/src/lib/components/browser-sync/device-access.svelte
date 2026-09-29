<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as Field from '$lib/components/ui/field';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let passphrase = $state('');
	let action = $state<'unlock' | 'enroll' | null>(null);

	function failureText(code: string, message: string): string {
		if (code === 'passphrase_rejected')
			return "That passphrase didn't unlock this browser.";
		return message || "This browser couldn't be set up.";
	}

	function enroll() {
		action = 'enroll';
		void panel
			.run(async (controller) => {
				const result = await controller.enroll({ passphrase });
				if (!result.ok) {
					panel.error = failureText(result.code, result.message);
					return;
				}
				passphrase = '';
				return 'Browser set up. Approve it from one of your other devices to share workspaces with it.';
			})
			.finally(() => {
				action = null;
			});
	}

	function unlock() {
		action = 'unlock';
		void panel
			.run(async (controller) => {
				const result = await controller.unlock({ passphrase });
				if (!result.ok) {
					panel.error = failureText(result.code, result.message);
					return;
				}
				passphrase = '';
				await panel.resume();
				return result.value === 'enrolled'
					? 'Unlocked for this tab.'
					: 'Unlocked. Set up this browser to sync.';
			})
			.finally(() => {
				action = null;
			});
	}
</script>

{#if panel.deviceId}
	<div class="flex flex-col gap-1">
		<p class="text-xs font-medium">Browser ID</p>
		<p class="font-mono text-xs break-all select-text">{panel.deviceId}</p>
	</div>
{/if}

{#if !panel.enrolled}
	<form
		onsubmit={(event) => {
			event.preventDefault();
			if (panel.deviceId) unlock();
			else enroll();
		}}
	>
		<Field.Group>
			<Field.Field>
				<Field.Label for="browser-sync-passphrase">Passphrase</Field.Label>
				<Input
					id="browser-sync-passphrase"
					type="password"
					autocomplete="current-password"
					bind:value={passphrase}
					disabled={panel.busy}
				/>
				<Field.Description
					>noura never stores your passphrase.</Field.Description
				>
			</Field.Field>
			<div class="flex flex-wrap gap-2">
				{#if panel.deviceId}
					<Button type="submit" disabled={panel.busy || !passphrase}>
						{#if action === 'unlock'}
							<Spinner data-icon="inline-start" />Unlocking…
						{:else}
							Unlock
						{/if}
					</Button>
					<Button
						type="button"
						variant="outline"
						disabled={panel.busy || !passphrase}
						onclick={enroll}
					>
						{#if action === 'enroll'}
							<Spinner data-icon="inline-start" />Setting up…
						{:else}
							Set up as a new browser
						{/if}
					</Button>
				{:else}
					<Button type="submit" disabled={panel.busy || !passphrase}>
						{#if action === 'enroll'}
							<Spinner data-icon="inline-start" />Setting up…
						{:else}
							Set up this browser
						{/if}
					</Button>
				{/if}
			</div>
		</Field.Group>
	</form>
{:else}
	<div class="flex flex-wrap gap-2">
		<Button
			variant="outline"
			disabled={panel.busy}
			onclick={() => {
				passphrase = '';
				panel.lock();
			}}>Lock</Button
		>
	</div>
{/if}
