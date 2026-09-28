<script lang="ts">
	import { tick, type Component } from 'svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { Input } from '$lib/components/ui/input';
	import { Button } from '$lib/components/ui/button';
	import { cn } from '$lib/utils';
	import { getAppPlatform } from '$lib/platform';
	import GearSix from 'phosphor-svelte/lib/GearSix';
	import TextAa from 'phosphor-svelte/lib/TextAa';
	import Keyboard from 'phosphor-svelte/lib/Keyboard';
	import UserCircle from 'phosphor-svelte/lib/UserCircle';
	import FolderSimple from 'phosphor-svelte/lib/FolderSimple';
	import PuzzlePiece from 'phosphor-svelte/lib/PuzzlePiece';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import PlugsConnected from 'phosphor-svelte/lib/PlugsConnected';
	import ArrowsClockwise from 'phosphor-svelte/lib/ArrowsClockwise';
	import Users from 'phosphor-svelte/lib/Users';
	import Files from 'phosphor-svelte/lib/Files';
	import Info from 'phosphor-svelte/lib/Info';
	import ArrowLeft from 'phosphor-svelte/lib/ArrowLeft';
	import GeneralSettings from './general-settings.svelte';
	import EditorSettings from './editor-settings.svelte';
	import ShortcutsSettings from './shortcuts-settings.svelte';
	import WorkspaceGeneralSettings from './workspace-general-settings.svelte';
	import PluginsSettings from './plugins-settings.svelte';
	import AiSettings from './ai-settings.svelte';
	import ExternalToolsSettings from './external-tools-settings.svelte';
	import FilesSettings from './files-settings.svelte';
	import AboutSettings from './about-settings.svelte';
	import SyncAccountSettings from '$lib/components/sync-account-settings.svelte';

	const settings = getSettingsDialog();
	const desktop = getAppPlatform() === 'desktop';

	type Section =
		| 'general'
		| 'editor'
		| 'shortcuts'
		| 'account'
		| 'workspace'
		| 'plugins'
		| 'ai'
		| 'external-tools'
		| 'sync'
		| 'people'
		| 'files'
		| 'about';
	type Entry = {
		id: Section;
		label: string;
		/** Shown under the heading only when it tells the reader something new. */
		description?: string;
		icon: Component;
		keywords?: string;
		desktopOnly?: boolean;
	};
	const groups: { label: string; entries: Entry[] }[] = [
		{
			label: 'This computer',
			entries: [
				{
					id: 'general',
					label: 'General',
					icon: GearSix,
					keywords:
						'appearance theme dark light system startup login updates version',
				},
				{
					id: 'editor',
					label: 'Editor',
					description:
						'These settings apply to every workspace on this computer.',
					icon: TextAa,
					keywords: 'text size font line width spelling spellcheck',
				},
				{
					id: 'shortcuts',
					label: 'Keyboard shortcuts',
					icon: Keyboard,
					keywords: 'keys',
				},
				{
					id: 'account',
					label: 'Account',
					icon: UserCircle,
					keywords: 'sign in log out disconnect server',
				},
			],
		},
		{
			label: 'Workspace',
			entries: [
				{
					id: 'workspace',
					label: 'Name and location',
					icon: FolderSimple,
					keywords: 'rename folder path',
				},
				{
					id: 'plugins',
					label: 'Plugins',
					description: 'Turn features on or off for this workspace.',
					icon: PuzzlePiece,
					keywords: 'notes tasks calendar projects folders',
				},
				{
					id: 'ai',
					label: 'AI',
					description:
						'Connect an AI provider and review what it can read in this workspace.',
					icon: Sparkle,
					keywords: 'providers models api keys permissions consent',
				},
				{
					id: 'external-tools',
					label: 'External tools',
					icon: PlugsConnected,
					keywords: 'mcp claude cursor assistant model context protocol',
					desktopOnly: true,
				},
				{
					id: 'sync',
					label: 'Sync and devices',
					icon: ArrowsClockwise,
					keywords: 'encryption recovery conflicts join devices',
				},
				{
					id: 'people',
					label: 'People',
					icon: Users,
					keywords: 'invitations roles sharing access',
				},
				{
					id: 'files',
					label: 'Ignored files',
					description:
						'noura won’t index or show files that match these patterns. They follow the same rules as .gitignore.',
					icon: Files,
					keywords: 'ignore gitignore exclude hidden files',
				},
			],
		},
		{
			label: 'App',
			entries: [
				{
					id: 'about',
					label: 'About noura',
					icon: Info,
					keywords: 'version license privacy terms',
				},
			],
		},
	];
	const visibleGroups = groups
		.map((group) => ({
			...group,
			entries: group.entries.filter((entry) => desktop || !entry.desktopOnly),
		}))
		.filter((group) => group.entries.length);

	let selected = $derived.by<Section>(() => {
		settings.requestId;
		return settings.requestedSection ?? 'general';
	});
	let query = $state('');
	let mobileContent = $state(true);
	let content: HTMLDivElement;
	let heading: HTMLHeadingElement;
	const current = $derived(
		visibleGroups
			.flatMap((group) => group.entries)
			.find((entry) => entry.id === selected)!,
	);
	const filtered = $derived(
		visibleGroups
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
			{#if current.description}
				<p class="text-sm leading-relaxed text-muted-foreground">
					{current.description}
				</p>
			{/if}
		</header>
		{#if selected === 'general'}
			<GeneralSettings />
		{:else if selected === 'editor'}
			<EditorSettings />
		{:else if selected === 'shortcuts'}
			<ShortcutsSettings />
		{:else if selected === 'workspace'}
			<WorkspaceGeneralSettings />
		{:else if selected === 'plugins'}
			<PluginsSettings onopenai={() => selectSection('ai')} />
		{:else if selected === 'ai'}
			<AiSettings />
		{:else if selected === 'external-tools'}
			<ExternalToolsSettings />
		{:else if selected === 'files'}
			<FilesSettings />
		{:else if selected === 'about'}
			<AboutSettings />
		{:else if selected === 'account' || selected === 'sync' || selected === 'people'}
			<SyncAccountSettings section={selected} />
		{/if}
	</div>
</div>
