<script lang="ts">
	import { untrack } from 'svelte';
	import { editorLinks } from '$lib/editor/links';
	import { createPointerSelectionTracker } from '$lib/editor/pointer-selection';
	import { recallView, rememberView } from '$lib/editor/view-memory';
	import { preferences } from '$lib/preferences.svelte';
	import type {
		EditorSelectionState,
		LiveMarkdownEditor,
		MarkdownFormat,
	} from '@noura/editor/types';
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
	import MagnifyingGlass from 'phosphor-svelte/lib/MagnifyingGlass';
	import ArrowCounterClockwise from 'phosphor-svelte/lib/ArrowCounterClockwise';
	import ArrowClockwise from 'phosphor-svelte/lib/ArrowClockwise';

	/**
	 * `value` seeds an editable surface once; after that the editor owns the
	 * text and changes only through the handle passed to `onready`, so a
	 * save finishing mid-typing can never reset what is on screen. Read-only
	 * surfaces follow `value`.
	 */
	let {
		value,
		sourceRelativePath,
		readOnly = false,
		showToolbar = true,
		label = 'Markdown editor',
		autofocus = false,
		memoryKey,
		onchange,
		onedit,
		onready,
	}: {
		value: string;
		sourceRelativePath?: string;
		readOnly?: boolean;
		showToolbar?: boolean;
		label?: string;
		/** Put the caret in the text once it opens. */
		autofocus?: boolean;
		/** Restore the caret and scroll position saved under this key. */
		memoryKey?: string;
		/** The user changed the text. Read it from the editor when needed. */
		onchange?: () => void;
		/** The user changed the text; receives the full text on every edit. */
		onedit?: (value: string) => void;
		onready?: (editor: LiveMarkdownEditor | null) => void;
	} = $props();

	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let selection = $state.raw<EditorSelectionState | null>(null);
	let selectingWithPointer = $state(false);
	let mountFailed = $state(false);
	let fallbackText = $state(untrack(() => value));

	// Keep the mounted CodeMirror view in step with the Settings toggle.
	$effect(() => {
		editor?.setSpellcheck(preferences.spellcheck);
	});

	// Read-only previews follow their source text.
	$effect(() => {
		if (readOnly) editor?.setText(value);
	});

	let popoverOpen = $derived(
		!readOnly &&
			!selectingWithPointer &&
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
	let formats = $derived(new Set(selection?.formats ?? []));
	const inlineFormats = [
		'bold',
		'italic',
		'underline',
		'strikethrough',
	] as const;
	let pressedInline = $derived(
		inlineFormats.filter((name) => formats.has(name)),
	);

	const isMac =
		typeof navigator !== 'undefined' &&
		/Mac|iPhone|iPad/.test(navigator.platform);
	const mod = isMac ? '⌘' : 'Ctrl+';

	const blockActions = [
		{ command: 'link', label: 'Link', shortcut: `${mod}K`, icon: LinkSimple },
		{ command: 'blockQuote', label: 'Quote', shortcut: '', icon: Quotes },
		{ command: 'code', label: 'Inline code', shortcut: '', icon: Code },
		{
			command: 'bulletList',
			label: 'Bulleted list',
			shortcut: '',
			icon: ListBullets,
		},
		{
			command: 'numberedList',
			label: 'Numbered list',
			shortcut: '',
			icon: ListNumbers,
		},
		{
			command: 'checkList',
			label: 'Checklist',
			shortcut: `${mod}Enter`,
			icon: Checks,
		},
	] as const;

	function format(command: MarkdownFormat) {
		editor?.format(command);
		editor?.focus();
	}

	function mountEditor(node: HTMLDivElement) {
		// Mount once: props read here must not re-run this attachment.
		const initial = untrack(() => ({
			value,
			readOnly,
			label,
			autofocus,
			memoryKey,
			sourceRelativePath,
			spellcheck: preferences.spellcheck,
		}));
		let disposed = false;
		let cleanup: (() => void) | undefined;
		void import('@noura/editor/runtime')
			.then(({ createLiveMarkdownEditor }) => {
				if (disposed) return;
				const pointerSelection = createPointerSelectionTracker(
					(active) => (selectingWithPointer = active),
				);
				const handle = createLiveMarkdownEditor(node, {
					text: initial.value,
					label: initial.label,
					readOnly: initial.readOnly,
					spellcheck: initial.spellcheck,
					restore: initial.memoryKey ? recallView(initial.memoryKey) : null,
					...editorLinks(initial.sourceRelativePath),
					onChange: () => {
						onchange?.();
						if (onedit) onedit(handle.doc());
					},
					onSelectionChange: (next) => {
						selection = next;
					},
				});
				const content = handle.view.contentDOM;
				content.addEventListener(
					'pointerdown',
					pointerSelection.pointerDown,
					true,
				);
				window.addEventListener(
					'pointermove',
					pointerSelection.pointerMove,
					true,
				);
				window.addEventListener('pointerup', pointerSelection.pointerUp, true);
				window.addEventListener(
					'pointercancel',
					pointerSelection.pointerCancel,
					true,
				);
				window.addEventListener('blur', pointerSelection.cancel);
				editor = handle;
				onready?.(handle);
				if (initial.autofocus && !initial.readOnly) handle.focus();
				cleanup = () => {
					// Read the key now: a renamed file keeps its place.
					const key = untrack(() => memoryKey);
					if (key) rememberView(key, handle.memory());
					content.removeEventListener(
						'pointerdown',
						pointerSelection.pointerDown,
						true,
					);
					window.removeEventListener(
						'pointermove',
						pointerSelection.pointerMove,
						true,
					);
					window.removeEventListener(
						'pointerup',
						pointerSelection.pointerUp,
						true,
					);
					window.removeEventListener(
						'pointercancel',
						pointerSelection.pointerCancel,
						true,
					);
					window.removeEventListener('blur', pointerSelection.cancel);
					pointerSelection.cancel();
					handle.destroy();
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
		return () => {
			disposed = true;
			cleanup?.();
			if (editor) {
				editor = null;
				onready?.(null);
			}
		};
	}
</script>

<div class="flex h-full min-h-0 flex-1 flex-col">
	{#if showToolbar && !readOnly}
		<div
			class="flex min-h-11 flex-wrap items-center gap-1 px-5 py-1.5"
			role="toolbar"
			aria-label="Formatting"
		>
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
			<ToggleGroup.Root
				type="multiple"
				size="sm"
				aria-label="Inline formatting"
				bind:value={() => [...pressedInline], () => {}}
			>
				<ToggleGroup.Item
					value="bold"
					onclick={() => format('bold')}
					aria-label="Bold ({mod}B)"><TextB /></ToggleGroup.Item
				>
				<ToggleGroup.Item
					value="italic"
					onclick={() => format('italic')}
					aria-label="Italic ({mod}I)"><TextItalic /></ToggleGroup.Item
				>
				<ToggleGroup.Item
					value="underline"
					onclick={() => format('underline')}
					aria-label="Underline ({mod}U)"><TextUnderline /></ToggleGroup.Item
				>
				<ToggleGroup.Item
					value="strikethrough"
					onclick={() => format('strikethrough')}
					aria-label="Strikethrough ({isMac ? '⇧⌘X' : 'Ctrl+Shift+X'})"
					><TextStrikethrough /></ToggleGroup.Item
				>
			</ToggleGroup.Root>
			<Separator orientation="vertical" class="mx-1 h-5" />
			{#each blockActions as action (action.command)}
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<Button
								{...props}
								variant="ghost"
								size="icon-sm"
								class="aria-pressed:bg-accent aria-pressed:text-accent-foreground"
								aria-pressed={formats.has(action.command)}
								onclick={() => format(action.command)}
								aria-label={action.label}><action.icon /></Button
							>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content
						>{action.label}{action.shortcut
							? ` (${action.shortcut})`
							: ''}</Tooltip.Content
					>
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
			<Button
				variant="ghost"
				size="icon-sm"
				class="ml-auto"
				onclick={() => editor?.find()}
				aria-label="Find and replace ({mod}F)"><MagnifyingGlass /></Button
			>
		</div>
		<Separator />
	{/if}

	<div class="relative min-h-0 flex-1 overflow-auto">
		{#if mountFailed}
			<textarea
				class="live-md h-full w-full resize-none bg-transparent outline-none"
				aria-label={label}
				readonly={readOnly}
				bind:value={fallbackText}
				oninput={(event) => {
					onchange?.();
					onedit?.(event.currentTarget.value);
				}}></textarea>
		{:else}
			<!-- CodeMirror's content element is the labelled text box. -->
			<div class="live-md h-full" {@attach mountEditor}></div>
		{/if}
		{#if popoverOpen && selection?.anchor}
			<Popover.Root bind:open={() => popoverOpen, () => undefined}>
				<Popover.Trigger
					aria-label="Selection formatting"
					class="fixed size-px opacity-0"
					style="left: {selection.anchor.left}px; top: {selection.anchor.top}px"
				/>
				<Popover.Content
					side="top"
					trapFocus={false}
					onOpenAutoFocus={(event) => event.preventDefault()}
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
</div>
