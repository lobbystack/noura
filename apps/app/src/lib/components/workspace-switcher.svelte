<script lang="ts">
	import { getSettingsDialog } from '$lib/settings.svelte';
	import GearSix from 'phosphor-svelte/lib/GearSix';
	const settings = getSettingsDialog();
	import { workspace } from '$lib/state.svelte';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { toast } from 'svelte-sonner';
	import Check from 'phosphor-svelte/lib/Check';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';

	let menuOpen = $state(false);

	function handleOpenChange(open: boolean) {
		menuOpen = open;
		if (open) void workspace.refreshRecents();
	}

	async function switchWorkspace(path: string, name: string) {
		if (workspace.isLoading) return;
		menuOpen = false;
		if (await workspace.open(path)) return;
		toast.error(`Could not open “${name}”`, {
			description:
				workspace.error ??
				'The folder may no longer be available or may not be a Noura workspace.',
		});
	}
</script>

<DropdownMenu.Root bind:open={menuOpen} onOpenChange={handleOpenChange}>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="ghost"
				size="sm"
				class="w-full min-w-0 justify-start"
				aria-label="Switch workspace. Current workspace: {workspace.name}"
			>
				<FolderOpen data-icon="inline-start" />
				<span class="min-w-0 flex-1 truncate text-left">{workspace.name}</span>
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>

	<DropdownMenu.Content
		class="w-80"
		align="start"
		onCloseAutoFocus={(event) => {
			if (settings.open) event.preventDefault();
		}}
	>
		<DropdownMenu.Label>Workspaces</DropdownMenu.Label>
		<DropdownMenu.Group>
			{#each workspace.recents as recent (recent.workspaceId)}
				{@const active = recent.workspaceId === workspace.state?.workspaceId}
				<DropdownMenu.Item
					disabled={active || workspace.isLoading}
					textValue={recent.name}
					onSelect={() => switchWorkspace(recent.path, recent.name)}
				>
					<FolderOpen />
					<span class="flex min-w-0 flex-1 flex-col gap-0.5">
						<span class="truncate font-medium">{recent.name}</span>
						<span class="truncate text-xs text-muted-foreground"
							>{recent.path}</span
						>
					</span>
					{#if active}
						<Check aria-label="Current workspace" weight="bold" />
					{/if}
				</DropdownMenu.Item>
			{/each}
		</DropdownMenu.Group>

		<DropdownMenu.Separator />
		<DropdownMenu.Group
			><DropdownMenu.Item
				onSelect={() => {
					menuOpen = false;
					settings.show();
				}}
				><GearSix />Settings<DropdownMenu.Shortcut>⌘,</DropdownMenu.Shortcut
				></DropdownMenu.Item
			></DropdownMenu.Group
		>
		<DropdownMenu.Separator />
		<DropdownMenu.Group>
			<DropdownMenu.Item
				disabled={workspace.isLoading}
				onSelect={() => {
					menuOpen = false;
					void workspace.pickAndOpen();
				}}
			>
				<Plus />
				Open another workspace…
			</DropdownMenu.Item>
		</DropdownMenu.Group>
	</DropdownMenu.Content>
</DropdownMenu.Root>
