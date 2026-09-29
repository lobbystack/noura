<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import AccountSection from '$lib/components/sync-account/account-section.svelte';
	import PeopleSection from '$lib/components/sync-account/people-section.svelte';
	import SyncSection from '$lib/components/sync-account/sync-section.svelte';
	import {
		SyncAccountSettings,
		type SyncSettingsSection,
	} from '$lib/sync-account-settings.svelte';

	let { section = 'account' }: { section?: SyncSettingsSection } = $props();

	const settings = new SyncAccountSettings();
	onMount(() => settings.mount(section));
</script>

<section
	aria-label={section === 'account'
		? 'noura account'
		: section === 'sync'
			? 'Workspace synchronization'
			: 'Workspace access'}
>
	<div class="flex flex-col gap-4">
		{#if settings.loading}<p
				role="status"
				class="text-sm text-muted-foreground"
			>
				Checking this device’s account…
			</p>
		{:else if settings.account}
			{#if section === 'account'}
				<AccountSection {settings} account={settings.account} />
			{:else if section === 'sync'}
				<SyncSection {settings} account={settings.account} />
			{:else if section === 'people'}
				<PeopleSection {settings} account={settings.account} />
			{/if}
		{:else if settings.request}
			<p class="text-sm">
				Approve this code in your browser to connect the device:
			</p>
			<p
				class="rounded-xl border p-4 text-center font-mono text-base tracking-widest"
				aria-label="Device sign-in code"
			>
				{settings.request.userCode}
			</p>
			<div class="flex flex-wrap gap-2">
				<Button
					disabled={settings.busy || settings.opening}
					onclick={() => settings.openBrowser()}
					>{settings.opening ? 'Opening…' : 'Open browser again'}</Button
				><Button
					variant="outline"
					disabled={settings.busy}
					onclick={() => settings.cancel()}>Cancel sign-in</Button
				>
			</div>
			<p class="text-xs text-muted-foreground">
				If the browser does not open, copy this address into your browser and
				enter the code above.
			</p>
			<p class="break-all text-xs select-text">
				{settings.request.verificationUri}
			</p>
			<p role="status" class="text-xs text-muted-foreground">
				Waiting for your approval. You can close Settings while you sign in.
			</p>
		{:else}
			<p class="text-sm text-muted-foreground">
				{#if section === 'sync'}
					Sync keeps an end-to-end encrypted copy of this workspace on your
					other devices. Sign in to turn it on.
				{:else if section === 'people'}
					Invite people to this workspace and choose what they can do. Sign in
					and turn on sync first.
				{:else}
					You only need an account for sync and sharing. Everything else works
					without one.
				{/if}
			</p>
			<div class="flex flex-wrap gap-2">
				<Button
					disabled={settings.busy ||
						(!settings.selfHosted && !settings.configuredOrigin) ||
						(settings.selfHosted && !settings.origin.trim())}
					onclick={() => settings.begin()}>Log in</Button
				>
				<Button
					variant="outline"
					disabled={settings.busy ||
						(!settings.selfHosted && !settings.configuredOrigin) ||
						(settings.selfHosted && !settings.origin.trim())}
					onclick={() => settings.begin(true)}>Sign up</Button
				>
			</div>
			{#if !settings.configuredOrigin && !settings.selfHosted}
				<p class="text-sm text-muted-foreground">
					This build has no default sync server. Connect to your own server
					instead.
				</p>
			{/if}
			<div>
				<Button
					variant="link"
					disabled={settings.busy}
					onclick={() => settings.toggleSelfHosted()}
					>Use a self-hosted server…</Button
				>
			</div>
			{#if settings.selfHosted}
				<Field.FieldGroup>
					<Field.Field>
						<Field.FieldLabel for="sync-server-origin"
							>Server address</Field.FieldLabel
						>
						<Input
							id="sync-server-origin"
							type="url"
							placeholder="https://…"
							autocomplete="off"
							bind:value={settings.origin}
							disabled={settings.busy}
						/>
						<Field.FieldDescription
							>The address where your noura server runs.</Field.FieldDescription
						>
					</Field.Field>
				</Field.FieldGroup>
			{/if}
		{/if}
		{#if settings.notice}<p role="status" class="text-xs text-muted-foreground">
				{settings.notice}
			</p>{/if}
		{#if settings.error}<p role="alert" class="text-sm text-destructive">
				{settings.error}
			</p>{/if}
	</div>
</section>
