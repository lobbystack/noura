<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { Input } from '$lib/components/ui/input/index.js';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import { cn } from '$lib/utils.js';

	/**
	 * A task name being typed: a new task before it exists, or a rename.
	 * Enter or leaving the field commits; Escape or an empty name cancels.
	 * A name still being typed when the app closes is committed first.
	 */
	let {
		value = '',
		label,
		placeholder = 'Task name',
		class: className,
		oncommit,
		oncancel,
	}: {
		value?: string;
		label: string;
		placeholder?: string;
		class?: string;
		/** Save `name` (trimmed, never empty). Resolves false when it failed. */
		oncommit: (name: string) => Promise<boolean>;
		oncancel: () => void;
	} = $props();

	let input = $state<HTMLInputElement | null>(null);
	// The field starts from `value` and then belongs to the person typing.
	let name = $state(untrack(() => value));
	let finished = false;

	async function finish(commit: boolean): Promise<boolean> {
		if (finished) return true;
		finished = true;
		const trimmed = name.trim();
		if (!commit || !trimmed || trimmed === value.trim()) {
			oncancel();
			return true;
		}
		const saved = await oncommit(trimmed);
		// A failed save keeps the field so the name can be saved again.
		if (!saved) finished = false;
		return saved;
	}

	onMount(() => {
		input?.focus();
		input?.select();
		return registerPendingDraft(
			`task-name:${crypto.randomUUID()}`,
			() => finish(true),
			() => !finished && name.trim() !== '' && name.trim() !== value.trim(),
		);
	});
</script>

<Input
	bind:ref={input}
	bind:value={name}
	aria-label={label}
	{placeholder}
	spellcheck="false"
	autocomplete="off"
	class={cn('h-7 px-2 text-sm', className)}
	onkeydown={(event) => {
		event.stopPropagation();
		if (event.key === 'Enter') {
			event.preventDefault();
			void finish(true);
		} else if (event.key === 'Escape') {
			event.preventDefault();
			void finish(false);
		}
	}}
	onblur={() => void finish(true)}
/>
