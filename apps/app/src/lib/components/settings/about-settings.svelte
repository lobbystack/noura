<script lang="ts">
	import { getAppUpdates } from '$lib/app-updates.svelte';
	import { getNouraClient } from '$lib/state.svelte';
	import { getAppPlatform } from '$lib/platform';
	import * as Item from '$lib/components/ui/item';
	import ArrowUpRight from 'phosphor-svelte/lib/ArrowUpRight';

	const updates = getAppUpdates();
	const client = getNouraClient();
	// The desktop webview can't open new windows, so links go to the browser.
	const desktop = getAppPlatform() === 'desktop';
	const repository = 'https://github.com/lobbystack/noura';

	const links = [
		{ label: 'Website', href: 'https://noura.app/' },
		{ label: 'Source code', href: repository },
		{ label: 'Report a problem', href: `${repository}/issues` },
		{ label: 'Privacy policy', href: 'https://noura.app/privacy/' },
		{ label: 'Terms and conditions', href: 'https://noura.app/terms/' },
		{
			label: 'Third-party licenses',
			href: `${repository}/blob/main/THIRD_PARTY_NOTICES.md`,
		},
	];

	function open(event: MouseEvent, href: string) {
		if (!desktop) return;
		event.preventDefault();
		void client.openLink(href);
	}
</script>

<div class="flex flex-col gap-8">
	<dl class="flex flex-col divide-y divide-border">
		<div class="flex justify-between gap-4 py-3">
			<dt class="text-sm">Version</dt>
			<dd class="text-sm text-muted-foreground">{updates.version ?? '—'}</dd>
		</div>
		<div class="flex justify-between gap-4 py-3">
			<dt class="text-sm">License</dt>
			<dd class="text-sm text-muted-foreground">MIT, free and open source</dd>
		</div>
	</dl>

	<Item.Group>
		{#each links as link (link.href)}
			<Item.Root variant="outline" size="sm">
				{#snippet child({ props })}
					<a
						{...props}
						href={link.href}
						target="_blank"
						rel="noopener noreferrer"
						onclick={(event) => open(event, link.href)}
					>
						<Item.Content><Item.Title>{link.label}</Item.Title></Item.Content>
						<Item.Actions><ArrowUpRight /></Item.Actions>
					</a>
				{/snippet}
			</Item.Root>
		{/each}
	</Item.Group>
</div>
