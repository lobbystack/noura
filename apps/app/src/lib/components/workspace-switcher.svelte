<script lang="ts">
	import { workspace } from '$lib/state.svelte';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import CaretDown from 'phosphor-svelte/lib/CaretDown';
	import Check from 'phosphor-svelte/lib/Check';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import Plus from 'phosphor-svelte/lib/Plus';

	let menuOpen = $state(false);

	function handleOpenChange(open: boolean) {
		menuOpen = open;
		if (open) void workspace.refreshRecents();
	}

	function switchWorkspace(path: string) {
		if (workspace.isLoading) return;
		menuOpen = false;
		void workspace.open(path);
	}
</script>

<DropdownMenu.Root bind:open={menuOpen} onOpenChange={handleOpenChange}>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="ghost"
				size="sm"
				class="max-w-64 text-foreground"
				aria-label="Switch workspace. Current workspace: {workspace.name}"
			>
				<FolderOpen data-icon="inline-start" />
				<span class="truncate">{workspace.name}</span>
				<CaretDown data-icon="inline-end" weight="bold" />
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>

	<DropdownMenu.Content class="w-80" align="start">
		<DropdownMenu.Label>Workspaces</DropdownMenu.Label>
		<DropdownMenu.Group>
			{#each workspace.recents as recent (recent.workspaceId)}
				{@const active = recent.workspaceId === workspace.state?.workspaceId}
				<DropdownMenu.Item
					disabled={active || workspace.isLoading}
					textValue={recent.name}
					onSelect={() => switchWorkspace(recent.path)}
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
