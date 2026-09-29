<script lang="ts">
	import { toast } from 'svelte-sonner';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { workspace } from '$lib/state.svelte';
	import { hostCapabilities } from '$lib/host-capabilities.svelte';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Check from 'phosphor-svelte/lib/Check';
	import FolderOpen from 'phosphor-svelte/lib/FolderOpen';
	import FolderPlus from 'phosphor-svelte/lib/FolderPlus';
	import ArrowSquareOut from 'phosphor-svelte/lib/ArrowSquareOut';
	import X from 'phosphor-svelte/lib/X';

	const settings = getSettingsDialog();
	// Desktop workspaces are folders; browser workspaces live in the browser.
	const folders = $derived(hostCapabilities.current.workspaceFolders);

	let menuOpen = $state(false);
	let createOpen = $state(false);
	let newName = $state('');

	const others = $derived(
		workspace.recents.filter(
			(recent) => recent.workspaceId !== workspace.state?.workspaceId,
		),
	);

	/** What the operating system calls its file manager. */
	const fileManager = /Mac/i.test(navigator.userAgent)
		? 'Finder'
		: /Windows/i.test(navigator.userAgent)
			? 'Explorer'
			: 'file manager';

	function handleOpenChange(open: boolean) {
		menuOpen = open;
		if (open) void workspace.refreshRecents();
	}

	async function forget(workspaceId: string, name: string) {
		try {
			await workspace.forgetRecent(workspaceId);
		} catch {
			toast.error(`Could not remove “${name}” from recents`);
		}
	}

	async function switchWorkspace(
		path: string,
		name: string,
		workspaceId: string,
	) {
		if (workspace.isLoading) return;
		menuOpen = false;
		if (await workspace.open(path)) return;
		toast.error(`Could not open “${name}”`, {
			description: 'You may have moved or deleted its folder.',
			action: {
				label: 'Remove from recents',
				onClick: () => void forget(workspaceId, name),
			},
		});
	}

	async function reveal() {
		try {
			await workspace.reveal();
		} catch {
			toast.error(`Could not open ${fileManager}`);
		}
	}

	async function createWorkspace(event: SubmitEvent) {
		event.preventDefault();
		const name = newName.trim();
		if (!name) return;
		createOpen = false;
		await workspace.createNamed(name);
		if (workspace.error)
			toast.error('Could not create the workspace', {
				description: workspace.error,
			});
		else newName = '';
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
			if (settings.open || createOpen) event.preventDefault();
		}}
	>
		<DropdownMenu.Group>
			<DropdownMenu.Label>Workspaces</DropdownMenu.Label>
			{#each workspace.recents as recent (recent.workspaceId)}
				{@const active = recent.workspaceId === workspace.state?.workspaceId}
				<DropdownMenu.Item
					disabled={active || workspace.isLoading}
					textValue={recent.name}
					onSelect={() =>
						switchWorkspace(recent.path, recent.name, recent.workspaceId)}
				>
					<FolderOpen />
					<span class="flex min-w-0 flex-1 flex-col gap-0.5">
						<span class="truncate font-medium">{recent.name}</span>
						{#if folders}
							<span class="truncate text-xs text-muted-foreground"
								>{recent.path}</span
							>
						{/if}
					</span>
					{#if active}
						<Check aria-label="Current workspace" weight="bold" />
					{/if}
				</DropdownMenu.Item>
			{/each}
		</DropdownMenu.Group>

		<DropdownMenu.Separator />
		<DropdownMenu.Group>
			{#if folders}
				<DropdownMenu.Item
					disabled={workspace.isLoading}
					onSelect={() => {
						menuOpen = false;
						void workspace.pickAndOpen();
					}}
				>
					<FolderOpen />
					Open folder…
				</DropdownMenu.Item>
			{/if}
			<DropdownMenu.Item
				disabled={workspace.isLoading}
				onSelect={() => {
					menuOpen = false;
					createOpen = true;
				}}
			>
				<FolderPlus />
				New workspace…
			</DropdownMenu.Item>
			{#if folders}
				<DropdownMenu.Item onSelect={() => void reveal()}>
					<ArrowSquareOut />
					Show in {fileManager}
				</DropdownMenu.Item>
			{/if}
			{#if folders && others.length > 0}
				<DropdownMenu.Sub>
					<DropdownMenu.SubTrigger>
						<X />
						Remove from recents
					</DropdownMenu.SubTrigger>
					<DropdownMenu.SubContent class="w-64">
						<DropdownMenu.Group>
							{#each others as recent (recent.workspaceId)}
								<DropdownMenu.Item
									textValue={recent.name}
									onSelect={() => void forget(recent.workspaceId, recent.name)}
								>
									<span class="truncate">{recent.name}</span>
								</DropdownMenu.Item>
							{/each}
						</DropdownMenu.Group>
					</DropdownMenu.SubContent>
				</DropdownMenu.Sub>
			{/if}
		</DropdownMenu.Group>
	</DropdownMenu.Content>
</DropdownMenu.Root>

<Dialog.Root bind:open={createOpen}>
	<Dialog.Content class="sm:max-w-sm">
		<Dialog.Header>
			<Dialog.Title>New workspace</Dialog.Title>
			<Dialog.Description>
				{folders ? 'Name it, then choose its folder.' : 'Name it.'}
			</Dialog.Description>
		</Dialog.Header>
		<form class="flex flex-col gap-4" onsubmit={createWorkspace}>
			<Field.Group>
				<Field.Field>
					<Field.Label for="switcher-new-workspace">Name</Field.Label>
					<Input
						id="switcher-new-workspace"
						bind:value={newName}
						autocomplete="off"
					/>
				</Field.Field>
			</Field.Group>
			<Dialog.Footer>
				<Button
					type="submit"
					disabled={workspace.isLoading || newName.trim().length === 0}
					>{folders ? 'Choose folder…' : 'Create'}</Button
				>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
