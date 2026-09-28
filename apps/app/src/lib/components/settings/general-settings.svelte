<script lang="ts">
	import { onMount } from 'svelte';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import { createTauriDesktopApp } from '@noura/workspace';
	import * as Field from '$lib/components/ui/field';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import { Switch } from '$lib/components/ui/switch';
	import { getAppPlatform } from '$lib/platform';
	import UpdatesSettings from './updates-settings.svelte';
	import Sun from 'phosphor-svelte/lib/Sun';
	import Moon from 'phosphor-svelte/lib/Moon';
	import Desktop from 'phosphor-svelte/lib/Desktop';

	const desktop = getAppPlatform() === 'desktop';
	const app = desktop ? createTauriDesktopApp() : null;

	let launchAtLogin = $state<boolean | null>(null);
	let launchError = $state('');

	onMount(() => {
		void app?.launchAtLogin
			.isEnabled()
			.then((enabled) => (launchAtLogin = enabled))
			.catch(() => (launchError = 'This system doesn’t allow changing this.'));
	});

	async function setLaunchAtLogin(enabled: boolean) {
		launchError = '';
		const previous = launchAtLogin;
		launchAtLogin = enabled;
		try {
			await app?.launchAtLogin.setEnabled(enabled);
		} catch {
			launchAtLogin = previous;
			launchError = 'Couldn’t change this setting. Try again.';
		}
	}
</script>

<Field.FieldGroup>
	{#if desktop}<UpdatesSettings />{/if}

	<Field.Field>
		<Field.FieldLabel id="appearance-label">Appearance</Field.FieldLabel>
		<ToggleGroup.Root
			type="single"
			variant="outline"
			aria-labelledby="appearance-label"
			value={userPrefersMode.current}
			onValueChange={(value) => {
				if (value === 'light' || value === 'dark' || value === 'system')
					setMode(value);
			}}
		>
			<ToggleGroup.Item value="light"><Sun />Light</ToggleGroup.Item>
			<ToggleGroup.Item value="dark"><Moon />Dark</ToggleGroup.Item>
			<ToggleGroup.Item value="system"><Desktop />Match system</ToggleGroup.Item
			>
		</ToggleGroup.Root>
	</Field.Field>

	{#if desktop}
		<Field.Field
			orientation="horizontal"
			data-disabled={launchAtLogin === null}
		>
			<Field.FieldContent>
				<Field.FieldLabel for="launch-at-login">Open at login</Field.FieldLabel>
				<Field.FieldDescription>
					Start noura when you sign in to this computer.
				</Field.FieldDescription>
				{#if launchError}<Field.FieldError>{launchError}</Field.FieldError>{/if}
			</Field.FieldContent>
			<Switch
				id="launch-at-login"
				checked={launchAtLogin ?? false}
				disabled={launchAtLogin === null}
				onCheckedChange={setLaunchAtLogin}
			/>
		</Field.Field>
	{/if}
</Field.FieldGroup>
