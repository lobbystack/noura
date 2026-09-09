<script lang="ts">
	import { getSettingsDialog } from '$lib/settings.svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import SettingsContent from './settings-content.svelte';

	const settings = getSettingsDialog();

	function handleKeydown(event: KeyboardEvent) {
		if (
			(event.metaKey || event.ctrlKey) &&
			event.key === ',' &&
			!event.repeat
		) {
			event.preventDefault();
			settings.show();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<Dialog.Root bind:open={settings.open}>
	<Dialog.Content
		class="flex h-[min(740px,calc(100svh-3rem))] w-[calc(100%-3rem)] max-w-[1040px] gap-0 overflow-hidden p-0 sm:max-w-[1040px] max-sm:h-svh max-sm:w-full max-sm:max-w-full max-sm:rounded-none"
		onCloseAutoFocus={(event) => {
			event.preventDefault();
			const target = settings.returnFocus;
			if (target?.isConnected && target !== document.body) target.focus();
			else document.getElementById('open-settings')?.focus();
		}}
	>
		<Dialog.Title class="sr-only">Settings</Dialog.Title>
		<Dialog.Description class="sr-only"
			>Manage plugins, your workspace, and this device.</Dialog.Description
		>
		{#if settings.open}
			<SettingsContent />
		{/if}
	</Dialog.Content>
</Dialog.Root>
