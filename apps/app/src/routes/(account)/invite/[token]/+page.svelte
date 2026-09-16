<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button';
	import { session, type Account } from '$account/auth';
	import {
		acceptWorkspaceInvitation,
		loadInvitation,
		type InvitationDetails,
	} from '$account/invitations';

	const token = $derived(page.params.token ?? '');
	let account = $state<Account | null>(null);
	let invitation = $state<InvitationDetails | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let accepted = $state(false);
	let error = $state('');
	const signInPath = $derived(`/account?invite=${encodeURIComponent(token)}`);

	onMount(() => {
		void Promise.all([session(), loadInvitation(token)])
			.then(([current, details]) => {
				account = current;
				invitation = details;
				accepted = details.accepted;
			})
			.catch((cause) => {
				error =
					cause instanceof Error
						? cause.message
						: 'This invitation is unavailable.';
			})
			.finally(() => {
				loading = false;
			});
	});

	async function accept() {
		busy = true;
		error = '';
		try {
			await acceptWorkspaceInvitation(token);
			accepted = true;
		} catch (cause) {
			error =
				cause instanceof Error
					? cause.message
					: 'The invitation could not be accepted.';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Workspace invitation · Noura</title></svelte:head>

<h1 class="text-base font-semibold">Workspace invitation</h1>
{#if loading}
	<p role="status">Checking this invitation…</p>
{:else if invitation}
	<p class="text-muted-foreground">
		You were invited to workspace <span
			class="break-all font-mono text-foreground">{invitation.workspaceId}</span
		>
		with the <strong class="text-foreground">{invitation.role}</strong> role.
	</p>
	{#if accepted}
		<p role="status">
			Invitation accepted. Connect or open a Noura desktop or browser device on
			this account, then ask the workspace owner to verify its fingerprint and
			finish granting access.
		</p>
	{:else if account}
		<p class="break-all text-sm text-muted-foreground">
			Accepting as {account.email}
		</p>
		<Button disabled={busy} onclick={() => void accept()}>
			{busy ? 'Accepting…' : 'Accept invitation'}
		</Button>
	{:else}
		<p>Sign in to choose the account that will receive this workspace role.</p>
		<Button href={signInPath}>Sign in to accept</Button>
	{/if}
{:else if !error}
	<p>This invitation is unavailable.</p>
{/if}
{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
