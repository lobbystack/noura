<script lang="ts">
	import { browser } from '$app/environment';
	import { fromAction } from 'svelte/attachments';
	import type {
		EditorSelectionState,
		LiveMarkdownEditor,
		MarkdownFormat,
	} from '@noura/editor/types';
	import { getNouraClient } from '$lib/state.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as ToggleGroup from '$lib/components/ui/toggle-group/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import TextB from 'phosphor-svelte/lib/TextB';
	import TextItalic from 'phosphor-svelte/lib/TextItalic';
	import TextUnderline from 'phosphor-svelte/lib/TextUnderline';
	import TextStrikethrough from 'phosphor-svelte/lib/TextStrikethrough';
	import LinkSimple from 'phosphor-svelte/lib/LinkSimple';
	import ListBullets from 'phosphor-svelte/lib/ListBullets';
	import ListNumbers from 'phosphor-svelte/lib/ListNumbers';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Quotes from 'phosphor-svelte/lib/Quotes';
	import Code from 'phosphor-svelte/lib/Code';
	import ArrowCounterClockwise from 'phosphor-svelte/lib/ArrowCounterClockwise';
	import ArrowClockwise from 'phosphor-svelte/lib/ArrowClockwise';

	let {
		value,
		sourceRelativePath,
		readOnly = false,
		showToolbar = true,
		label = 'Markdown editor',
		onedit,
		onready,
	}: {
		value: string;
		sourceRelativePath?: string;
		readOnly?: boolean;
		showToolbar?: boolean;
		label?: string;
		onedit?: (value: string) => void;
		onready?: (editor: LiveMarkdownEditor | null) => void;
	} = $props();

	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let selection = $state.raw<EditorSelectionState | null>(null);
	let mountFailed = $state(false);
	let popoverOpen = $derived(
		!readOnly &&
			(selection?.hasFocus ?? false) &&
			!(selection?.composing ?? false) &&
			(selection?.to ?? 0) > (selection?.from ?? 0) &&
			selection?.anchor !== null,
	);

	const blockStyles = [
		{ value: 'text', label: 'Text' },
		{ value: 'heading1', label: 'Heading 1' },
		{ value: 'heading2', label: 'Heading 2' },
		{ value: 'heading3', label: 'Heading 3' },
	] as const;
	let blockStyle = $derived(selection?.blockStyle ?? 'text');
	let blockStyleLabel = $derived(
		blockStyles.find((option) => option.value === blockStyle)?.label ?? 'Text',
	);

	function format(command: MarkdownFormat) {
		editor?.format(command);
		editor?.focus();
	}

	function mountEditor(node: HTMLDivElement, initialValue: string) {
		if (!browser) return;
		let disposed = false;
		let cleanup: (() => void) | undefined;
		let latestValue = initialValue;
		void import('@noura/editor/runtime')
			.then(({ createLiveMarkdownDocument, createLiveMarkdownEditor }) => {
				if (disposed) return;
				const document = createLiveMarkdownDocument('markdown', latestValue);
				const handle = createLiveMarkdownEditor(node, {
					ytext: document.ytext,
					collaborative: !readOnly,
					readOnly,
					resolveImage: sourceRelativePath
						? (target) =>
								getNouraClient()
									.files.readLocalAsset({ sourceRelativePath, target })
									.then((asset) => asset.dataUrl)
									.catch(() => null)
						: undefined,
					resolveLink: sourceRelativePath
						? async (target) => {
								const resolved =
									await getNouraClient().files.resolveMarkdownLink({
										sourceRelativePath,
										target,
									});
								if (resolved.kind === 'managed') {
									return {
										kind: 'managed' as const,
										label: resolved.object.title,
										preview: resolved.object.body,
									};
								}
								if (resolved.kind === 'markdown') {
									return {
										kind: 'markdown' as const,
										label: target,
										preview: resolved.document.body,
									};
								}
								return resolved;
							}
						: undefined,
					onChange: () => onedit?.(handle.doc()),
					onSelectionChange: (next) => {
						selection = next;
					},
				});
				editor = handle;
				onready?.(handle);
				cleanup = () => {
					handle.destroy();
					document.destroy();
				};
			})
			.catch((error: unknown) => {
				// A failed editor mount must never leave a blank surface: fall
				// back to a plain textarea over the same value so the file stays
				// editable, and keep the details in the local console.
				console.error('live markdown editor failed to mount', error);
				if (!disposed) {
					mountFailed = true;
					onready?.(null);
				}
			});
		return {
			update(next: string) {
				latestValue = next;
				if (editor && editor.doc() !== next) editor.setText(next);
			},
			destroy() {
				disposed = true;
				cleanup?.();
				if (editor) {
					editor = null;
					onready?.(null);
				}
			},
		};
	}
</script>

{#if showToolbar && !readOnly}
	<div class="flex min-h-11 items-center gap-1 overflow-x-auto px-5 py-1.5">
		<Select.Root
			type="single"
			value={blockStyle}
			onValueChange={(next) => next && format(next as MarkdownFormat)}
		>
			<Select.Trigger size="sm" class="w-28" aria-label="Paragraph style"
				>{blockStyleLabel}</Select.Trigger
			>
			<Select.Content>
				<Select.Group>
					{#each blockStyles as option (option.value)}
						<Select.Item value={option.value} label={option.label}
							>{option.label}</Select.Item
						>
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
		<Separator orientation="vertical" class="mx-1 h-5" />
		<ToggleGroup.Root type="multiple" size="sm" aria-label="Inline formatting">
			<ToggleGroup.Item
				value="bold"
				onclick={() => format('bold')}
				aria-label="Bold"><TextB /></ToggleGroup.Item
			>
			<ToggleGroup.Item
				value="italic"
				onclick={() => format('italic')}
				aria-label="Italic"><TextItalic /></ToggleGroup.Item
			>
			<ToggleGroup.Item
				value="underline"
				onclick={() => format('underline')}
				aria-label="Underline"><TextUnderline /></ToggleGroup.Item
			>
			<ToggleGroup.Item
				value="strike"
				onclick={() => format('strikethrough')}
				aria-label="Strikethrough"><TextStrikethrough /></ToggleGroup.Item
			>
		</ToggleGroup.Root>
		<Separator orientation="vertical" class="mx-1 h-5" />
		{#each [{ command: 'link', label: 'Link', icon: LinkSimple }, { command: 'blockQuote', label: 'Quote', icon: Quotes }, { command: 'code', label: 'Inline code', icon: Code }, { command: 'bulletList', label: 'Bulleted list', icon: ListBullets }, { command: 'numberedList', label: 'Numbered list', icon: ListNumbers }, { command: 'checkList', label: 'Checklist', icon: Checks }] as action (action.command)}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<Button
							{...props}
							variant="ghost"
							size="icon-sm"
							onclick={() => format(action.command as MarkdownFormat)}
							aria-label={action.label}><action.icon /></Button
						>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content>{action.label}</Tooltip.Content>
			</Tooltip.Root>
		{/each}
		<Separator orientation="vertical" class="mx-1 h-5" />
		<Button
			variant="ghost"
			size="icon-sm"
			onclick={() => editor?.undo()}
			aria-label="Undo"><ArrowCounterClockwise /></Button
		>
		<Button
			variant="ghost"
			size="icon-sm"
			onclick={() => editor?.redo()}
			aria-label="Redo"><ArrowClockwise /></Button
		>
	</div>
	<Separator />
{/if}

<div class="relative min-h-0 flex-1">
	{#if mountFailed}
		<textarea
			class="live-md min-h-full w-full resize-none bg-transparent text-base outline-none"
			aria-label={label}
			readonly={readOnly}
			{value}
			oninput={(event) => onedit?.(event.currentTarget.value)}></textarea>
	{:else}
		<div
			class="live-md min-h-full text-base"
			role="textbox"
			aria-label={label}
			aria-readonly={readOnly}
			{@attach fromAction(mountEditor, () => value)}
		></div>
	{/if}
	{#if popoverOpen && selection?.anchor}
		<Popover.Root open={popoverOpen}>
			<Popover.Trigger
				aria-label="Selection formatting"
				class="fixed size-px opacity-0"
				style="left: {selection.anchor.left}px; top: {selection.anchor.top}px"
			/>
			<Popover.Content
				side="top"
				class="w-auto flex-row gap-1 p-1"
				onpointerdown={(event) => event.preventDefault()}
			>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => format('bold')}
					aria-label="Bold selection"><TextB /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => format('italic')}
					aria-label="Italic selection"><TextItalic /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => format('underline')}
					aria-label="Underline selection"><TextUnderline /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => format('strikethrough')}
					aria-label="Strike selection"><TextStrikethrough /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => format('link')}
					aria-label="Link selection"><LinkSimple /></Button
				>
			</Popover.Content>
		</Popover.Root>
	{/if}
</div>
