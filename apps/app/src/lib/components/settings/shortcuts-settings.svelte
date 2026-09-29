<script lang="ts">
	import { Kbd, KbdGroup } from '$lib/components/ui/kbd';
	import { APP_SHORTCUTS } from '$lib/app-shortcuts';
	import { hostOs, shortcutKeys } from '$lib/host-os';

	const os = hostOs();

	const groups: { label: string; shortcuts: [string, string[]][] }[] = [
		{
			label: 'Anywhere',
			shortcuts: [
				...APP_SHORTCUTS.map(
					({ label, keys }) => [label, keys] as [string, string[]],
				),
				['Close a dialog', ['Esc']],
			],
		},
		{
			label: 'File tree',
			shortcuts: [
				['Move up and down', ['↑', '↓']],
				['Open or close a folder', ['←', '→']],
				['Open a file', ['Enter']],
				['Rename', ['F2']],
				['Move to the trash', os === 'mac' ? ['Mod', 'Backspace'] : ['Delete']],
				['Show the menu for a file', ['Shift', 'F10']],
				['Jump to a name by typing it', ['A–Z']],
			],
		},
		{
			label: 'Tabs',
			shortcuts: [
				['Move between tabs', ['←', '→']],
				['Close the focused tab', ['Delete']],
			],
		},
		{
			label: 'Writing',
			shortcuts: [
				['Save now', ['Mod', 'S']],
				['Undo', ['Mod', 'Z']],
				['Redo', ['Mod', 'Shift', 'Z']],
				['Indent a list item', ['Tab']],
				['Outdent a list item', ['Shift', 'Tab']],
			],
		},
		{
			label: 'AI chat',
			shortcuts: [['Send a message', ['Mod', 'Enter']]],
		},
		{
			label: 'PDF search',
			shortcuts: [
				['Next match', ['Enter']],
				['Previous match', ['Shift', 'Enter']],
			],
		},
	];
</script>

<div class="flex flex-col gap-8">
	{#each groups as group (group.label)}
		<section class="flex flex-col gap-1" aria-label={group.label}>
			<h3 class="text-xs text-muted-foreground">{group.label}</h3>
			<dl class="flex flex-col divide-y divide-border">
				{#each group.shortcuts as [label, keys] (label)}
					<div class="flex items-center justify-between gap-4 py-3">
						<dt class="text-sm">{label}</dt>
						<dd>
							<KbdGroup>
								{#each shortcutKeys(keys, os) as key, index (index)}<Kbd
										>{key}</Kbd
									>{/each}
							</KbdGroup>
						</dd>
					</div>
				{/each}
			</dl>
		</section>
	{/each}
</div>
