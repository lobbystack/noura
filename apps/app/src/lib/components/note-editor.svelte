<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { Editor } from '@tiptap/core';
	import StarterKit from '@tiptap/starter-kit';
	import TaskList from '@tiptap/extension-task-list';
	import TaskItem from '@tiptap/extension-task-item';
	import { Markdown } from '@tiptap/markdown';
	import type { CoreEvent, Note } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
	import {
		saveNoteWithReconciliation,
		type NoteDraft,
	} from '$lib/editor/note-save';
	import { reconcileNoteTitle } from '$lib/editor/title-reconciliation';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Toggle } from '$lib/components/ui/toggle/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import TextB from 'phosphor-svelte/lib/TextB';
	import TextItalic from 'phosphor-svelte/lib/TextItalic';
	import TextUnderline from 'phosphor-svelte/lib/TextUnderline';
	import ListBullets from 'phosphor-svelte/lib/ListBullets';
	import ListNumbers from 'phosphor-svelte/lib/ListNumbers';
	import Checks from 'phosphor-svelte/lib/Checks';
	import Quotes from 'phosphor-svelte/lib/Quotes';
	import Code from 'phosphor-svelte/lib/Code';
	import ArrowCounterClockwise from 'phosphor-svelte/lib/ArrowCounterClockwise';
	import ArrowClockwise from 'phosphor-svelte/lib/ArrowClockwise';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		note,
		onsaved,
	}: {
		note: Note | null;
		onsaved?: (updated: Note) => void;
	} = $props();

	type ConflictState = { draft: NoteDraft; file: Note };
	type Resolution = 'use-external' | 'replace-external';
	type BlockStyle = 'paragraph' | 'heading-1' | 'heading-2' | 'heading-3';

	const blockStyles: { value: BlockStyle; label: string }[] = [
		{ value: 'paragraph', label: 'Text' },
		{ value: 'heading-1', label: 'Heading 1' },
		{ value: 'heading-2', label: 'Heading 2' },
		{ value: 'heading-3', label: 'Heading 3' },
	];

	const initialNote = () => note;
	let currentNote = $state.raw<Note | null>(initialNote());
	let draftTitle = $state(initialNote()?.title ?? '');
	let baseBody = $state(initialNote()?.body ?? '');
	let baseTitle = $state(initialNote()?.title ?? '');
	let baseRevision = $state(initialNote()?.revision ?? '');
	let editor = $state.raw<Editor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<NoteDraft> | null>(null);
	let autosaveError = $state<unknown | null>(null);
	let conflict = $state.raw<ConflictState | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);
	let transientMessage = $state<string | null>(null);
	let toolbarRevision = $state(0);
	let blockStyle = $derived.by<BlockStyle>(() => {
		toolbarRevision;
		if (editor?.isActive('heading', { level: 1 })) return 'heading-1';
		if (editor?.isActive('heading', { level: 2 })) return 'heading-2';
		if (editor?.isActive('heading', { level: 3 })) return 'heading-3';
		return 'paragraph';
	});
	let blockStyleLabel = $derived(
		blockStyles.find((option) => option.value === blockStyle)?.label ?? 'Text',
	);
	let clearMessageTimer: ReturnType<typeof setTimeout> | null = null;

	function errorMessage(error: unknown) {
		if (error instanceof Error) return error.message;
		if (error && typeof error === 'object' && 'message' in error) {
			return String(error.message);
		}
		return 'Noura could not save this note.';
	}

	function showMessage(message: string) {
		transientMessage = message;
		if (clearMessageTimer) clearTimeout(clearMessageTimer);
		clearMessageTimer = setTimeout(() => {
			transientMessage = null;
			clearMessageTimer = null;
		}, 2400);
	}

	function currentDraft(): NoteDraft {
		return {
			title: draftTitle,
			body: editor?.getMarkdown() ?? coordinator?.getDraft()?.body ?? baseBody,
		};
	}

	function replaceEditorBody(body: string, preserveSelection = true) {
		if (!editor || editor.getMarkdown() === body) return;
		const selection = editor.state.selection;
		editor.commands.setContent(body, {
			contentType: 'markdown',
			emitUpdate: false,
		});
		if (preserveSelection) {
			const maximum = editor.state.doc.content.size;
			editor.commands.setTextSelection({
				from: Math.min(selection.from, maximum),
				to: Math.min(selection.to, maximum),
			});
		}
	}

	function setCanonical(value: Note) {
		currentNote = value;
		baseBody = value.body;
		baseTitle = value.title;
		baseRevision = value.revision;
		onsaved?.(value);
	}

	async function persistDraft(draft: NoteDraft, generation: number) {
		const target = currentNote;
		if (!target) return;
		const result = await saveNoteWithReconciliation(
			target,
			{ revision: baseRevision, title: baseTitle, body: baseBody },
			draft,
		);
		if (result.status === 'conflict') {
			openConflict(result.draft, result.current);
			return 'paused' as const;
		}

		const merged = result.value.body !== draft.body;
		setCanonical(result.value);
		if (coordinator?.currentGeneration === generation) {
			draftTitle = result.value.title;
			replaceEditorBody(result.value.body);
		}
		if (merged) showMessage('External changes merged');
	}

	async function flushNow() {
		return (await coordinator?.flush()) ?? true;
	}

	function isActive(name: string) {
		toolbarRevision;
		return editor?.isActive(name) ?? false;
	}

	function applyBlockStyle(value: string) {
		if (!editor) return;
		const chain = editor.chain().focus();
		if (value === 'paragraph') chain.setParagraph().run();
		else if (value === 'heading-1') chain.setHeading({ level: 1 }).run();
		else if (value === 'heading-2') chain.setHeading({ level: 2 }).run();
		else if (value === 'heading-3') chain.setHeading({ level: 3 }).run();
	}

	function editorContainer(node: HTMLDivElement) {
		if (!browser || !currentNote) return;
		const localCoordinator = new AutosaveCoordinator({
			write: persistDraft,
			onStateChange: (state) => {
				autosaveError = state.error;
			},
		});
		coordinator = localCoordinator;
		const localEditor = new Editor({
			element: node,
			extensions: [
				StarterKit,
				TaskList,
				TaskItem.configure({ nested: true }),
				Markdown,
			],
			content: currentNote.body,
			contentType: 'markdown',
			editorProps: {
				attributes: {
					class: 'min-h-full outline-none',
					'aria-label': `Edit ${currentNote.title}`,
				},
			},
			onUpdate: ({ editor: updatedEditor }) => {
				localCoordinator.noteEdit({
					title: draftTitle,
					body: updatedEditor.getMarkdown(),
				});
				toolbarRevision += 1;
			},
			onSelectionUpdate: () => {
				toolbarRevision += 1;
			},
			onBlur: () => {
				void localCoordinator.flush();
			},
		});
		editor = localEditor;
		const unregister = registerPendingDraft(
			currentNote.id,
			() => localCoordinator.flush(),
			() =>
				localCoordinator.pendingEdits > 0 ||
				localCoordinator.isWriting ||
				localCoordinator.error !== null,
		);

		return () => {
			unregister();
			localCoordinator.destroy();
			localEditor.destroy();
			if (coordinator === localCoordinator) coordinator = null;
			if (editor === localEditor) editor = null;
		};
	}

	function openConflict(draft: NoteDraft, file: Note) {
		conflict = { draft, file };
		reviewOpen = true;
		coordinator?.pause();
	}

	async function handleExternalEvent(event: CoreEvent) {
		const target = currentNote;
		if (
			!target ||
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			!['object:updated', 'object:moved', 'object:deleted'].includes(event.type)
		)
			return;
		const payload = event.payload as { id?: string };
		if (payload.id !== target.id) return;
		if (event.type === 'object:deleted') {
			openConflict(currentDraft(), target);
			return;
		}

		const latest = await getNouraClient().notes.get(target.id);
		if (!coordinator?.pendingEdits && !coordinator?.isWriting) {
			setCanonical(latest);
			draftTitle = latest.title;
			replaceEditorBody(latest.body);
			return;
		}
		const draft = currentDraft();
		const reconciliation = await getNouraClient().notes.reconcileDraft({
			id: target.id,
			baseRevision,
			baseBody,
			localBody: draft.body,
		});
		if (reconciliation.status === 'conflict') {
			openConflict(draft, reconciliation.current as Note);
			return;
		}
		const canonical = reconciliation.current as Note;
		const title = reconcileNoteTitle(baseTitle, draft.title, canonical.title);
		if (title.status === 'conflict') {
			openConflict(draft, canonical);
			return;
		}
		currentNote = canonical;
		baseBody = canonical.body;
		baseTitle = canonical.title;
		baseRevision = canonical.revision;
		draftTitle = title.title;
		if (reconciliation.status === 'merged') {
			replaceEditorBody(reconciliation.body);
			showMessage('External changes merged');
		}
		coordinator?.noteEdit(currentDraft());
	}

	function requestResolution(resolution: Resolution) {
		pendingResolution = resolution;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !pendingResolution || !currentNote) return;
		resolving = true;
		try {
			const localDraft = currentDraft();
			const result = await getNouraClient().notes.resolveConflict({
				id: currentNote.id,
				currentRevision: conflict.file.revision,
				localBody: localDraft.body,
				resolution: pendingResolution,
			});
			let resolved = result.value as Note;
			if (
				pendingResolution === 'replace-external' &&
				resolved.title !== localDraft.title
			) {
				const renamed = await getNouraClient().notes.update(resolved.id, {
					expectedRevision: resolved.revision,
					title: localDraft.title,
					body: resolved.body,
				});
				resolved = renamed.value as Note;
			}
			setCanonical(resolved);
			if (pendingResolution === 'use-external') {
				draftTitle = resolved.title;
				replaceEditorBody(resolved.body, false);
			} else {
				draftTitle = localDraft.title;
			}
			coordinator?.acceptDurable();
			coordinator?.resume();
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			showMessage(
				pendingResolution === 'use-external'
					? 'File version restored'
					: 'File replaced',
			);
			pendingResolution = null;
		} catch (error) {
			autosaveError = error;
		} finally {
			resolving = false;
		}
	}

	function handleShortcut(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
			event.preventDefault();
			void flushNow();
		}
	}

	onMount(() => {
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		if (browser) {
			void getNouraClient()
				.events.subscribe((event) => void handleExternalEvent(event))
				.then((unlisten) => {
					if (disposed) unlisten();
					else unsubscribe = unlisten;
				});
		}
		return () => {
			disposed = true;
			unsubscribe?.();
			if (clearMessageTimer) clearTimeout(clearMessageTimer);
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

{#if !currentNote}
	<EmptyState
		icon={NotePencil}
		title="Select a note"
		description="Choose a note from the list or create a new one."
	/>
{:else}
	<div class="flex min-h-0 flex-1 flex-col">
		<header class="flex min-h-16 items-center px-6">
			<input
				bind:value={draftTitle}
				oninput={() => coordinator?.noteEdit(currentDraft())}
				onblur={() => void flushNow()}
				aria-label="Note title"
				placeholder="Untitled"
				class="min-w-0 flex-1 bg-transparent text-lg font-semibold outline-none placeholder:text-muted-foreground"
			/>
		</header>
		<Separator />

		<div class="flex min-h-11 items-center gap-1 overflow-x-auto px-5 py-1.5">
			<Select.Root
				type="single"
				value={blockStyle}
				onValueChange={applyBlockStyle}
			>
				<Select.Trigger size="sm" class="w-28" aria-label="Paragraph style">
					{blockStyleLabel}
				</Select.Trigger>
				<Select.Content>
					<Select.Group>
						{#each blockStyles as option (option.value)}
							<Select.Item value={option.value} label={option.label}>
								{option.label}
							</Select.Item>
						{/each}
					</Select.Group>
				</Select.Content>
			</Select.Root>
			<Separator orientation="vertical" class="mx-1 h-5" />
			<Toggle
				size="sm"
				pressed={isActive('bold')}
				onPressedChange={() => editor?.chain().focus().toggleBold().run()}
				aria-label="Bold"
			>
				<TextB />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('italic')}
				onPressedChange={() => editor?.chain().focus().toggleItalic().run()}
				aria-label="Italic"
			>
				<TextItalic />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('underline')}
				onPressedChange={() => editor?.chain().focus().toggleUnderline().run()}
				aria-label="Underline"
			>
				<TextUnderline />
			</Toggle>
			<Separator orientation="vertical" class="mx-1 h-5" />
			<Toggle
				size="sm"
				pressed={isActive('bulletList')}
				onPressedChange={() => editor?.chain().focus().toggleBulletList().run()}
				aria-label="Bulleted list"
			>
				<ListBullets />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('orderedList')}
				onPressedChange={() =>
					editor?.chain().focus().toggleOrderedList().run()}
				aria-label="Numbered list"
			>
				<ListNumbers />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('taskList')}
				onPressedChange={() => editor?.chain().focus().toggleTaskList().run()}
				aria-label="Checklist"
			>
				<Checks />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('blockquote')}
				onPressedChange={() => editor?.chain().focus().toggleBlockquote().run()}
				aria-label="Quote"
			>
				<Quotes />
			</Toggle>
			<Toggle
				size="sm"
				pressed={isActive('codeBlock')}
				onPressedChange={() => editor?.chain().focus().toggleCodeBlock().run()}
				aria-label="Code block"
			>
				<Code />
			</Toggle>
			<Separator orientation="vertical" class="mx-1 h-5" />
			<Button
				variant="ghost"
				size="icon-sm"
				onclick={() => editor?.chain().focus().undo().run()}
				disabled={!editor?.can().undo()}
				aria-label="Undo"
			>
				<ArrowCounterClockwise />
			</Button>
			<Button
				variant="ghost"
				size="icon-sm"
				onclick={() => editor?.chain().focus().redo().run()}
				disabled={!editor?.can().redo()}
				aria-label="Redo"
			>
				<ArrowClockwise />
			</Button>
			{#if transientMessage}
				<span class="ml-auto text-xs text-muted-foreground" role="status">
					{transientMessage}
				</span>
			{/if}
		</div>
		<Separator />

		{#if conflict}
			<div class="px-6 pt-4">
				<Alert.Root>
					<Warning />
					<Alert.Title>This note changed in another app</Alert.Title>
					<Alert.Description>
						Your draft is safe. Review both versions before choosing which one
						to keep.
					</Alert.Description>
					<Alert.Action>
						<Button
							variant="outline"
							size="sm"
							onclick={() => (reviewOpen = true)}
						>
							Review conflict
						</Button>
					</Alert.Action>
				</Alert.Root>
			</div>
		{/if}

		{#if autosaveError}
			<div class="px-6 pt-4">
				<Alert.Root variant="destructive">
					<Warning />
					<Alert.Title>Changes could not be saved</Alert.Title>
					<Alert.Description>{errorMessage(autosaveError)}</Alert.Description>
					<Alert.Action>
						<Button variant="outline" size="sm" onclick={() => void flushNow()}>
							Retry
						</Button>
					</Alert.Action>
				</Alert.Root>
			</div>
		{/if}

		<div class="min-h-0 flex-1 overflow-y-auto">
			<div class="mx-auto min-h-full max-w-3xl px-10 py-10">
				<div
					{@attach editorContainer}
					class="
						min-h-full text-base leading-7 [&_.ProseMirror]:min-h-[60vh]
						[&_.ProseMirror_h1]:mb-4 [&_.ProseMirror_h1]:text-3xl [&_.ProseMirror_h1]:font-semibold
						[&_.ProseMirror_h2]:mt-7 [&_.ProseMirror_h2]:mb-3 [&_.ProseMirror_h2]:text-2xl [&_.ProseMirror_h2]:font-semibold
						[&_.ProseMirror_h3]:mt-6 [&_.ProseMirror_h3]:mb-2 [&_.ProseMirror_h3]:text-xl [&_.ProseMirror_h3]:font-semibold
						[&_.ProseMirror_p]:mb-3 [&_.ProseMirror_ul]:my-3 [&_.ProseMirror_ul]:pl-6
						[&_.ProseMirror_ol]:my-3 [&_.ProseMirror_ol]:pl-6
						[&_.ProseMirror_blockquote]:my-4 [&_.ProseMirror_blockquote]:border-l-2 [&_.ProseMirror_blockquote]:border-border [&_.ProseMirror_blockquote]:pl-4 [&_.ProseMirror_blockquote]:text-muted-foreground
						[&_.ProseMirror_pre]:my-4 [&_.ProseMirror_pre]:overflow-x-auto [&_.ProseMirror_pre]:rounded-md [&_.ProseMirror_pre]:bg-muted [&_.ProseMirror_pre]:p-4
						[&_.ProseMirror_a]:underline [&_.ProseMirror_a]:underline-offset-4
					"
				></div>
			</div>
		</div>
	</div>

	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl">
			<Sheet.Header>
				<Sheet.Title>Review note conflict</Sheet.Title>
				<Sheet.Description>
					Noura preserved both versions. Choose which note should remain in the
					Markdown file.
				</Sheet.Description>
			</Sheet.Header>
			{#if conflict}
				<div
					class="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-5 overflow-y-auto px-4 pb-4"
				>
					<section class="min-w-0">
						<h2 class="mb-3 text-sm font-medium">Your draft</h2>
						<p class="mb-4 truncate text-base font-semibold">
							{conflict.draft.title || 'Untitled'}
						</p>
						<MarkdownPreview markdown={conflict.draft.body} />
					</section>
					<Separator orientation="vertical" />
					<section class="min-w-0">
						<h2 class="mb-3 text-sm font-medium">File version</h2>
						<p class="mb-4 truncate text-base font-semibold">
							{conflict.file.title || 'Untitled'}
						</p>
						<MarkdownPreview markdown={conflict.file.body} />
					</section>
				</div>
			{/if}
			<Sheet.Footer>
				<Button variant="outline" onclick={() => (reviewOpen = false)}>
					Cancel and continue reviewing
				</Button>
				<Button
					variant="outline"
					onclick={() => requestResolution('use-external')}
				>
					Use file version
				</Button>
				<Button onclick={() => requestResolution('replace-external')}>
					Replace file with my version
				</Button>
			</Sheet.Footer>
		</Sheet.Content>
	</Sheet.Root>

	<AlertDialog.Root bind:open={confirmOpen}>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>
					{pendingResolution === 'use-external'
						? 'Use the file version?'
						: 'Replace the file version?'}
				</AlertDialog.Title>
				<AlertDialog.Description>
					{pendingResolution === 'use-external'
						? 'Your visible draft will be saved to recovery history before the file version replaces it.'
						: 'The current file version will be saved to recovery history before your draft replaces it.'}
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel disabled={resolving}>Cancel</AlertDialog.Cancel>
				<AlertDialog.Action
					disabled={resolving}
					onclick={() => void resolveConflict()}
				>
					Continue
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>
{/if}
