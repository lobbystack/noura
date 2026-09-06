<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';
	import {
		session,
		reviewDevice,
		decideDevice,
		accountPath,
		userCode,
		type Account,
		type DeviceReview,
	} from '$account/auth';
	let account = $state<Account | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let code = $state('');
	let review = $state<DeviceReview | null>(null);
	let error = $state('');
	let result = $state('');
	async function run(action: () => Promise<void>) {
		busy = true;
		error = '';
		try {
			await action();
		} catch (cause) {
			error =
				cause instanceof Error
					? cause.message
					: 'Unable to connect. Please try again.';
		} finally {
			busy = false;
		}
	}
	async function inspect() {
		review = await reviewDevice(code);
	}
	async function decide(decision: 'approve' | 'deny') {
		if (!review) return;
		await decideDevice(review.user_code, decision);
		review = null;
		result =
			decision === 'approve'
				? 'Desktop sign-in approved. Return to Noura desktop to finish connecting.'
				: 'Desktop sign-in denied. You can close this page.';
	}
	onMount(() => {
		code = userCode(new URLSearchParams(location.search).get('user_code'));
		void run(async () => {
			account = await session();
			if (account && code) await inspect();
		}).finally(() => {
			loading = false;
		});
	});
</script>

<h1 class="text-2xl font-semibold">Connect Noura desktop</h1>
<p class="text-muted-foreground">
	Only approve if you started signing in from Noura desktop. Compare this code
	with the code on your device.
</p>
{#if loading}<p role="status">Checking the sign-in request…</p>
{:else if !account}<Button href={accountPath(code)}
		>Sign in to review this device</Button
	>
{:else if result}<p role="status">{result}</p>
{:else if review}
	<p class="break-all text-sm text-muted-foreground">
		Signing in as {account.email}
	</p>
	<p
		class="rounded-xl border p-4 text-center font-mono text-2xl tracking-widest"
		aria-label="Device code"
	>
		{review.user_code}
	</p>
	{#if review.status === 'pending'}
		<p class="text-sm">
			Approving lets this desktop request an account session. Workspace
			encryption keys are transferred separately between your authorized
			devices.
		</p>
		<div class="flex flex-wrap gap-3">
			<Button disabled={busy} onclick={() => run(() => decide('approve'))}
				>Approve this desktop</Button
			><Button
				variant="outline"
				disabled={busy}
				onclick={() => run(() => decide('deny'))}>Deny</Button
			>
		</div>
	{:else}<p role="status">
			This sign-in request has already been {review.status}. Return to Noura
			desktop.
		</p>{/if}
{:else}
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void run(inspect);
		}}
	>
		<Field.Group
			><Field.Field
				><Field.Label for="code">Device code</Field.Label><Input
					id="code"
					autocomplete="off"
					maxlength={12}
					required
					bind:value={code}
					disabled={busy}
				/></Field.Field
			><Button type="submit" disabled={busy}>Review request</Button
			></Field.Group
		>
	</form>
{/if}
{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
