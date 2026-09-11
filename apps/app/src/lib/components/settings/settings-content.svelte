<script lang="ts">
	import { tick, type Component } from 'svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	const settings = getSettingsDialog();
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { cn } from '$lib/utils';
	import PuzzlePiece from 'phosphor-svelte/lib/PuzzlePiece';
	import GearSix from 'phosphor-svelte/lib/GearSix';
	import UserCircle from 'phosphor-svelte/lib/UserCircle';
	import ArrowsClockwise from 'phosphor-svelte/lib/ArrowsClockwise';
	import Users from 'phosphor-svelte/lib/Users';
	import Wrench from 'phosphor-svelte/lib/Wrench';
	import Info from 'phosphor-svelte/lib/Info';
	import ArrowLeft from 'phosphor-svelte/lib/ArrowLeft';
	import Keyboard from 'phosphor-svelte/lib/Keyboard';
	import PreferencesSettings from './preferences-settings.svelte';
	import SlidersHorizontal from 'phosphor-svelte/lib/SlidersHorizontal';
	import PluginsSettings from './plugins-settings.svelte';
	import WorkspaceSettings from './workspace-settings.svelte';
	import SyncAccountSettings from '$lib/components/sync-account-settings.svelte';
	import appPackage from '../../../../../../package.json';

	type Section =
		| 'preferences'
		| 'plugins'
		| 'general'
		| 'sync'
		| 'people'
		| 'account'
		| 'advanced'
		| 'about'
		| 'shortcuts';
	type Entry = {
		id: Section;
		label: string;
		description: string;
		icon: Component;
		keywords?: string;
	};
	const groups: { label: string; entries: Entry[] }[] = [
		{
			label: 'Personal',
			entries: [
				{
					id: 'preferences',
					label: 'Preferences',
					description: 'Choose an appearance for this device.',
					icon: SlidersHorizontal,
					keywords: 'appearance theme dark light system',
				},
				{
					id: 'account',
					label: 'Account',
					description: 'Manage your Noura account and this device.',
					icon: UserCircle,
				},
				{
					id: 'shortcuts',
					label: 'Keyboard shortcuts',
					description: 'Shortcuts for search, settings, and dialogs.',
					icon: Keyboard,
				},
			],
		},
		{
			label: 'Workspace',
			entries: [
				{
					id: 'plugins',
					label: 'Plugins',
					description: 'Enable plugins and configure them for this workspace.',
					icon: PuzzlePiece,
					keywords:
						'AI providers models credentials permissions notes tasks calendar projects folders',
				},
				{
					id: 'general',
					label: 'General',
					description: 'View the workspace name and folder location.',
					icon: GearSix,
				},
				{
					id: 'sync',
					label: 'Sync & devices',
					description: 'Manage synchronization, trusted devices, and recovery.',
					icon: ArrowsClockwise,
					keywords: 'encryption recovery conflicts',
				},
				{
					id: 'people',
					label: 'People & access',
					description: 'Create invitations and review workspace access.',
					icon: Users,
					keywords: 'invitations roles sharing',
				},
			],
		},
		{
			label: 'Application',
			entries: [
				{
					id: 'advanced',
					label: 'Advanced',
					description: 'Check diagnostics and rebuild the search index.',
					icon: Wrench,
					keywords: 'diagnostics rebuild issues',
				},
				{
					id: 'about',
					label: 'About Noura',
					description: 'Version and license information.',
					icon: Info,
					keywords: 'version license',
				},
			],
		},
	];
	let selected = $derived.by<Section>(() => {
		settings.requestId;
		return settings.requestedSection ?? 'plugins';
	});
	let query = $state('');
	let mobileContent = $state(true);
	let content: HTMLDivElement;
	let heading: HTMLHeadingElement;
	const current = $derived(
		groups
			.flatMap((group) => group.entries)
			.find((entry) => entry.id === selected)!,
	);
	const filtered = $derived(
		groups
			.map((group) => ({
				...group,
				entries: group.entries.filter((entry) =>
					`${entry.label} ${entry.keywords ?? ''}`
						.toLowerCase()
						.includes(query.trim().toLowerCase()),
				),
			}))
			.filter((group) => group.entries.length),
	);

	async function selectSection(id: Section) {
		selected = id;
		mobileContent = true;
		await tick();
		content?.scrollTo(0, 0);
		heading?.focus();
	}
</script>

<aside
	class={cn(
		'flex w-[220px] shrink-0 flex-col gap-5 border-r border-border bg-sidebar p-4 max-sm:w-full max-sm:border-r-0',
		mobileContent && 'max-sm:hidden',
	)}
>
	<div class="flex flex-col gap-4 pt-1">
		<p class="px-2 font-semibold">Settings</p>
		<Input
			aria-label="Search settings"
			placeholder="Search settings…"
			bind:value={query}
		/>
	</div>
	<nav
		aria-label="Settings sections"
		class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto"
	>
		{#each filtered as group (group.label)}
			<div class="flex flex-col gap-1">
				<p class="px-2 pb-1 text-xs text-muted-foreground">{group.label}</p>
				{#each group.entries as entry (entry.id)}
					<Button
						variant={selected === entry.id ? 'secondary' : 'ghost'}
						class="w-full justify-start gap-2"
						aria-current={selected === entry.id ? 'page' : undefined}
						onclick={() => selectSection(entry.id)}
					>
						<entry.icon data-icon="inline-start" />
						{entry.label}
					</Button>
				{/each}
			</div>
		{:else}
			<p class="px-2 text-sm text-muted-foreground" role="status">
				No settings found.
			</p>
		{/each}
	</nav>
</aside>

<div
	{@attach (node) => {
		content = node;
	}}
	class={cn(
		'min-w-0 flex-1 overflow-y-auto overscroll-contain px-6 py-8 sm:px-10 sm:py-10',
		!mobileContent && 'max-sm:hidden',
	)}
>
	<div class="mx-auto flex max-w-[680px] flex-col gap-8">
		<header class="flex flex-col gap-2 pr-6">
			<div class="sm:hidden">
				<Button
					variant="ghost"
					size="sm"
					onclick={() => {
						mobileContent = false;
					}}><ArrowLeft data-icon="inline-start" />Settings</Button
				>
			</div>
			<h1
				{@attach (node) => {
					heading = node;
				}}
				tabindex="-1"
				class="text-base font-semibold outline-none"
			>
				{current.label}
			</h1>
			<p class="text-sm leading-relaxed text-muted-foreground">
				{current.description}
			</p>
		</header>
		{#if selected === 'plugins'}
			<PluginsSettings />
		{:else if selected === 'preferences'}
			<PreferencesSettings />
		{:else if selected === 'general' || selected === 'advanced'}
			<WorkspaceSettings section={selected} />
		{:else if selected === 'account' || selected === 'sync' || selected === 'people'}
			<SyncAccountSettings section={selected} />
		{:else if selected === 'shortcuts'}
			<dl class="flex flex-col divide-y divide-border">
				{#each [['Open settings', '⌘ / Ctrl ,'], ['Search and commands', '⌘ / Ctrl K'], ['Close dialog', 'Esc']] as [label, shortcut] (label)}
					<div class="flex items-center justify-between gap-4 py-4">
						<dt>{label}</dt>
						<dd>
							<kbd
								class="rounded-md border border-border px-2 py-1 text-xs text-muted-foreground"
								>{shortcut}</kbd
							>
						</dd>
					</div>
				{/each}
			</dl>
		{:else if selected === 'about'}
			<div class="flex flex-col gap-6">
				<dl class="flex flex-col divide-y divide-border">
					<div class="flex justify-between py-4">
						<dt>Version</dt>
						<dd>{appPackage.version ?? '0.1.0'}</dd>
					</div>
					<div class="flex justify-between py-4">
						<dt>License</dt>
						<dd>MIT</dd>
					</div>
				</dl>
			</div>
		{/if}
	</div>
</div>
