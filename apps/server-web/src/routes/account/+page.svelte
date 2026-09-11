<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';
	import {
		session,
		sendMagicLink,
		signUpPasskey,
		signInPasskey,
		addPasskey,
		signOut,
		userCode,
		devicePath,
		invitationPath,
		type Account,
	} from '$account/auth';
	let signup = $state(false);
	let account = $state<Account | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let email = $state('');
	let code = $state('');
	let invite = $state('');
	let notice = $state('');
	let error = $state('');
	async function run(action: () => Promise<void>) {
		busy = true;
		error = '';
		notice = '';
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
	onMount(() => {
		const query = new URLSearchParams(location.search);
		signup = query.get('mode') === 'signup';
		code = userCode(query.get('user_code'));
		invite = invitationPath(query.get('invite'));
		void run(async () => {
			account = await session();
			if (account && code) location.assign(devicePath(code));
			else if (account && invite) location.assign(invite);
		}).finally(() => {
			loading = false;
		});
	});
</script>

<h1 class="text-2xl font-semibold">
	{account
		? 'Your account'
		: signup
			? 'Create your Noura account'
			: 'Log in to Noura'}
</h1>
{#if loading}<p role="status">Checking your session…</p>
{:else if account}
	<p class="break-all text-muted-foreground">Signed in as {account.email}</p>
	{#if code}<Button href={devicePath(code)}>Review desktop sign-in</Button>{/if}
	{#if invite}<Button href={invite}>Review workspace invitation</Button>{/if}
	<Button
		disabled={busy}
		onclick={() =>
			run(async () => {
				await addPasskey();
				notice = 'Passkey added. You can use it next time you sign in.';
			})}>Add a passkey</Button
	>
	<Button
		variant="outline"
		disabled={busy}
		onclick={() =>
			run(async () => {
				await signOut();
				account = null;
			})}>Sign out</Button
	>
{:else}
	<p class="text-muted-foreground">
		Create an account with a passkey, or sign in to connect your desktop.
	</p>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void run(async () => {
				await sendMagicLink(email, code, invite);
				notice =
					'If this email is invited to this server, a sign-in link is on its way. Check your inbox.';
			});
		}}
	>
		<Field.Group
			><Field.Field
				><Field.Label for="email">Email address</Field.Label><Input
					id="email"
					type="email"
					autocomplete="email"
					required
					bind:value={email}
					disabled={busy}
				/></Field.Field
			><Button
				type="button"
				disabled={busy}
				onclick={() =>
					run(async () => {
						await signUpPasskey(email, code, invite);
						notice =
							'Passkey created. Check your email to finish creating your account.';
					})}>{busy ? 'Please wait…' : 'Create account with a passkey'}</Button
			><Button type="submit" variant="outline" disabled={busy}
				>Email me a sign-in link</Button
			></Field.Group
		>
	</form>
	<Button
		variant="outline"
		disabled={busy}
		onclick={() =>
			run(async () => {
				await signInPasskey();
				account = await session();
				if (account && code) location.assign(devicePath(code));
				else if (account && code) location.assign(devicePath(code));
				else if (account && invite) location.assign(invite);
			})}>Use an existing passkey</Button
	>
{/if}
{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
