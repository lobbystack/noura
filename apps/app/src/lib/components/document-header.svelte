<script lang="ts">
	import { normalizeFileName, splitFileName } from '$lib/editor/rename';

	/**
	 * The header of every Markdown document: the file name, which is the
	 * title. Editing the name renames the file when the
	 * edit is committed (Enter or leaving the field); typing never renames.
	 */
	let {
		relativePath,
		disabled = false,
		autofocus = false,
		onrename,
		ondone,
	}: {
		relativePath: string;
		disabled?: boolean;
		/** Select the name for typing, as for a new note. */
		autofocus?: boolean;
		/** Rename to `name` (already normalized). Rejects when it failed. */
		onrename: (name: string) => Promise<void>;
		/** Enter was pressed: move on to the text. */
		ondone?: () => void;
	} = $props();

	const parts = $derived(splitFileName(relativePath));
	const markdown = $derived(parts.extension.toLowerCase() === '.md');
	/** What is being typed; null while the field shows the file name. */
	let draft = $state<string | null>(null);
	let renaming = $state(false);

	async function commit() {
		if (draft === null) return;
		const name = normalizeFileName(draft);
		if (!name || name === parts.stem) {
			draft = null;
			return;
		}
		renaming = true;
		try {
			await onrename(name);
		} catch {
			// The caller explains what went wrong; show the current name again.
		} finally {
			renaming = false;
			draft = null;
		}
	}

	function handleKeydown(
		event: KeyboardEvent & { currentTarget: HTMLInputElement },
	) {
		if (event.key === 'Enter') {
			event.preventDefault();
			event.currentTarget.blur();
			ondone?.();
		} else if (event.key === 'Escape') {
			event.preventDefault();
			draft = null;
			event.currentTarget.blur();
		}
	}

	let nameField: HTMLInputElement | null = null;

	function focusName(input: HTMLInputElement) {
		nameField = input;
		if (!autofocus) return;
		input.focus();
		input.select();
	}

	/**
	 * F2 renames the open document from anywhere, including its text: a
	 * click in the file tree opens the file and puts the caret in the text,
	 * where the tree's own F2 no longer applies. Other text fields and
	 * dialogs keep the key.
	 */
	function renameShortcut(event: KeyboardEvent) {
		if (
			event.key !== 'F2' ||
			event.defaultPrevented ||
			event.metaKey ||
			event.ctrlKey ||
			event.altKey ||
			event.shiftKey
		)
			return;
		const field = nameField;
		if (!field || field.disabled) return;
		const target = event.target instanceof HTMLElement ? event.target : null;
		const inText = target?.closest('.cm-editor') !== null;
		if (
			target &&
			!inText &&
			(target.closest('[role="dialog"], [role="alertdialog"]') ||
				target instanceof HTMLInputElement ||
				target instanceof HTMLTextAreaElement ||
				target.isContentEditable)
		)
			return;
		event.preventDefault();
		field.focus();
		field.select();
	}
</script>

<svelte:window onkeydown={renameShortcut} />

<header class="flex min-h-12 flex-col justify-center px-6 py-2">
	<div class="flex min-w-0 items-baseline">
		<input
			{@attach focusName}
			value={draft ?? parts.stem}
			oninput={(event) => (draft = event.currentTarget.value)}
			onblur={() => void commit()}
			onkeydown={handleKeydown}
			disabled={disabled || renaming}
			aria-label="File name"
			placeholder="Untitled"
			spellcheck="false"
			autocomplete="off"
			class="min-w-0 flex-1 truncate bg-transparent text-base font-semibold outline-none placeholder:text-muted-foreground disabled:opacity-100"
		/>
		{#if !markdown && parts.extension}
			<span class="shrink-0 text-base text-muted-foreground"
				>{parts.extension}</span
			>
		{/if}
	</div>
</header>
