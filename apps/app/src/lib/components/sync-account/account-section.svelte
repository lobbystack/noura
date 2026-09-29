<script lang="ts">
	import type { SyncAccount } from '@noura/workspace';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import type { SyncAccountSettings } from '$lib/sync-account-settings.svelte';

	let {
		settings,
		account,
	}: { settings: SyncAccountSettings; account: SyncAccount } = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3">
	<p class="break-all text-sm">{account.origin}</p>
	<Badge variant="secondary">Signed in</Badge>
</div>
<div class="flex flex-col gap-1">
	<p class="text-xs font-medium">Device code</p>
	<p class="break-all font-mono text-xs select-text">
		{account.fingerprint}
	</p>
	<p class="text-xs text-muted-foreground">
		When you approve this computer from another device, check that both show
		this code.
	</p>
</div>
<div>
	<Button
		variant="outline"
		disabled={settings.busy ||
			settings.exportingRecovery ||
			settings.importingRecovery ||
			settings.syncChanging ||
			settings.devicesBusy ||
			settings.joining ||
			settings.remoteBusy}
		onclick={() => settings.disconnect()}
		>{settings.busy ? 'Disconnecting…' : 'Disconnect this device'}</Button
	>
</div>
