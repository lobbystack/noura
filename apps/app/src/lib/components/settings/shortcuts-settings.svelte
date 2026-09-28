<script lang="ts">
	import { browser } from '$app/environment';
	import { Kbd, KbdGroup } from '$lib/components/ui/kbd';

	// Display only: the handlers accept both Command and Control.
	const mac =
		import.meta.env.NOURA_TAURI_PLATFORM === 'darwin' ||
		(browser && /Mac|iPhone|iPad/.test(navigator.userAgent));
	const mod = mac ? '⌘' : 'Ctrl';
	const shift = mac ? '⇧' : 'Shift';

	const groups: { label: string; shortcuts: [string, string[]][] }[] = [
		{
			label: 'Anywhere',
			shortcuts: [
				['Search and run commands', [mod, 'K']],
				['Open settings', [mod, ',']],
				['Close a dialog', ['Esc']],
			],
		},
		{
			label: 'Writing',
			shortcuts: [
				['Save now', [mod, 'S']],
				['Undo', [mod, 'Z']],
				['Redo', [mod, shift, 'Z']],
				['Indent a list item', ['Tab']],
				['Outdent a list item', [shift, 'Tab']],
			],
		},
		{
			label: 'AI chat',
			shortcuts: [['Send a message', [mod, 'Enter']]],
		},
		{
			label: 'PDF search',
			shortcuts: [
				['Next match', ['Enter']],
				['Previous match', [shift, 'Enter']],
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
								{#each keys as key (key)}<Kbd>{key}</Kbd>{/each}
							</KbdGroup>
						</dd>
					</div>
				{/each}
			</dl>
		</section>
	{/each}
</div>
