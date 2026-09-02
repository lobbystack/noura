<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type { CoreEvent, Note } from '@noura/workspace';
	import { isCoreError } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
	import {
		saveNoteWithReconciliation,
		type NoteDraft,
	} from '$lib/editor/note-save';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		note,
		onsaved,
		autofocusTitle = false,
	}: {
		note: Note | null;
		onsaved?: (updated: Note) => void;
		/** Freshly created notes start with the title focused and selected. */
		autofocusTitle?: boolean;
	} = $props();

	let titleInput = $state<HTMLInputElement | null>(null);

	type ConflictState = { draft: NoteDraft; file: Note; deleted?: boolean };
	type Resolution = 'use-external' | 'replace-external';

	const initialNote = () => note;
	let currentNote = $state.raw<Note | null>(initialNote());
	let draftTitle = $state(initialNote()?.title ?? '');
	let baseBody = $state(initialNote()?.body ?? '');
	let baseTitle = $state(initialNote()?.title ?? '');
	let baseRevision = $state(initialNote()?.revision ?? '');
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<NoteDraft> | null>(null);
	let autosaveError = $state<unknown | null>(null);
	let conflict = $state.raw<ConflictState | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);
	let transientMessage = $state<string | null>(null);
	let editorCleanup: (() => void) | null = null;
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
			body: editor?.doc() ?? coordinator?.getDraft()?.body ?? baseBody,
		};
	}

	function replaceEditorBody(body: string) {
		editor?.setText(body);
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

	function connectEditor(handle: LiveMarkdownEditor | null) {
		editorCleanup?.();
		editorCleanup = null;
		editor = handle;
		if (!handle || !currentNote) return;
		const localCoordinator = new AutosaveCoordinator({
			write: persistDraft,
			onStateChange: (state) => {
				autosaveError = state.error;
			},
		});
		coordinator = localCoordinator;
		const unregister = registerPendingDraft(
			currentNote.id,
			() => localCoordinator.flush(),
			() =>
				localCoordinator.pendingEdits > 0 ||
				localCoordinator.isWriting ||
				localCoordinator.error !== null,
		);

		editorCleanup = () => {
			unregister();
			localCoordinator.destroy();
			if (coordinator === localCoordinator) coordinator = null;
		};
	}

	function handleEditorEdit(body: string) {
		coordinator?.noteEdit({ title: draftTitle, body });
	}

	function openConflict(draft: NoteDraft, file: Note, deleted = false) {
		conflict = { draft, file, deleted };
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
			openConflict(currentDraft(), target, true);
			return;
		}

		try {
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
			let mergedTitle = canonical.title;
			if (draft.title !== baseTitle && canonical.title === baseTitle) {
				mergedTitle = draft.title;
			} else if (
				draft.title !== baseTitle &&
				canonical.title !== baseTitle &&
				draft.title !== canonical.title
			) {
				openConflict(draft, canonical);
				return;
			}
			currentNote = canonical;
			baseBody = canonical.body;
			baseTitle = canonical.title;
			baseRevision = canonical.revision;
			draftTitle = mergedTitle;
			if (reconciliation.status === 'merged') {
				const mergedBody = reconciliation.body;
				replaceEditorBody(mergedBody);
				showMessage('External changes merged');
			}
			coordinator?.noteEdit(currentDraft());
		} catch (error) {
			if (isCoreError(error) && error.code === 'object_not_found') {
				// The file disappeared between the event and the fetch; show the
				// same review surface the deleted event would.
				openConflict(currentDraft(), target, true);
				return;
			}
			autosaveError = error;
		}
	}

	function requestResolution(resolution: Resolution) {
		pendingResolution = resolution;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !pendingResolution || !currentNote) return;
		const pendingConflict = conflict;
		if (pendingConflict.deleted && pendingResolution === 'use-external') return;
		resolving = true;
		try {
			const localDraft = pendingConflict.draft;
			const resolved = (await getNouraClient().notes.resolveManagedConflict({
				id: currentNote.id,
				currentRevision: pendingConflict.file.revision,
				relativePath: currentNote.relativePath,
				created: currentNote.created,
				localTitle: localDraft.title,
				localBody: localDraft.body,
				localProperties: currentNote.properties,
				resolution: pendingResolution,
			})) as Note;
			setCanonical(resolved);
			if (pendingResolution === 'use-external') {
				draftTitle = resolved.title;
				replaceEditorBody(resolved.body);
			} else {
				draftTitle = localDraft.title;
			}
			coordinator?.acceptDurable();
			coordinator?.resume();
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			showMessage(
				pendingConflict.deleted
					? 'Note restored'
					: pendingResolution === 'use-external'
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

	$effect(() => {
		// The notes page remounts this editor per note; a fresh note starts
		// with its placeholder title fully selected so typing renames it.
		// Consume the flag: setCanonical reassigns currentNote on every
		// autosave, and the pending focus must not steal back from the body.
		if (autofocusTitle && titleInput && currentNote) {
			autofocusTitle = false;
			titleInput.focus();
			titleInput.select();
		}
	});

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
			editorCleanup?.();
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

{#if !currentNote}
	<EmptyState
		icon={NotePencil}
		title="No document open"
		description="Pick a document from the sidebar, or create a new note."
	/>
{:else}
	<div class="flex min-h-0 flex-1 flex-col">
		<header class="flex min-h-16 items-center px-6">
			<input
				bind:this={titleInput}
				bind:value={draftTitle}
				oninput={() => coordinator?.noteEdit(currentDraft())}
				onblur={() => void flushNow()}
				aria-label="Note title"
				placeholder="Untitled"
				class="min-w-0 flex-1 bg-transparent text-lg font-semibold outline-none placeholder:text-muted-foreground"
			/>
		</header>
		<Separator />

		{#if conflict}
			<div class="px-6 pt-4">
				<Alert.Root>
					<Warning />
					<Alert.Title
						>{conflict.deleted
							? 'This note file was deleted'
							: 'This note changed in another app'}</Alert.Title
					>
					<Alert.Description>
						{conflict.deleted
							? 'Your draft is safe and can restore the deleted Markdown file.'
							: 'Your draft is safe. Review both versions before choosing which one to keep.'}
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

		{#if transientMessage}
			<p class="px-6 pt-3 text-xs text-muted-foreground" role="status">
				{transientMessage}
			</p>
		{/if}

		<div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
			<LiveMarkdownSurface
				value={baseBody}
				sourceRelativePath={currentNote.relativePath}
				label="Note body"
				onedit={handleEditorEdit}
				onready={connectEditor}
			/>
		</div>
	</div>

	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl">
			<Sheet.Header>
				<Sheet.Title>Review note conflict</Sheet.Title>
				<Sheet.Description>
					{conflict?.deleted
						? 'The Markdown file was deleted outside Noura. Restore it from your preserved draft.'
						: 'Noura preserved both versions. Choose which note should remain in the Markdown file.'}
				</Sheet.Description>
			</Sheet.Header>
			{#if conflict}
				<div
					class="grid min-h-0 flex-1 gap-5 overflow-y-auto px-4 pb-4 {conflict.deleted
						? 'grid-cols-1'
						: 'grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]'}"
				>
					<section class="min-w-0">
						<h2 class="mb-3 text-sm font-medium">Your draft</h2>
						<p class="mb-4 truncate text-base font-semibold">
							{conflict.draft.title || 'Untitled'}
						</p>
						<MarkdownPreview markdown={conflict.draft.body} />
					</section>
					{#if !conflict.deleted}
						<Separator orientation="vertical" />
						<section class="min-w-0">
							<h2 class="mb-3 text-sm font-medium">File version</h2>
							<p class="mb-4 truncate text-base font-semibold">
								{conflict.file.title || 'Untitled'}
							</p>
							<MarkdownPreview markdown={conflict.file.body} />
						</section>
					{/if}
				</div>
			{/if}
			<Sheet.Footer>
				<Button variant="outline" onclick={() => (reviewOpen = false)}>
					Cancel and continue reviewing
				</Button>
				{#if !conflict?.deleted}
					<Button
						variant="outline"
						onclick={() => requestResolution('use-external')}
					>
						Use file version
					</Button>
				{/if}
				<Button onclick={() => requestResolution('replace-external')}>
					{conflict?.deleted
						? 'Restore note file'
						: 'Replace file with my version'}
				</Button>
			</Sheet.Footer>
		</Sheet.Content>
	</Sheet.Root>

	<AlertDialog.Root bind:open={confirmOpen}>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>
					{conflict?.deleted
						? 'Restore the note file?'
						: pendingResolution === 'use-external'
							? 'Use the file version?'
							: 'Replace the file version?'}
				</AlertDialog.Title>
				<AlertDialog.Description>
					{conflict?.deleted
						? 'Your preserved draft will be written back to its original Markdown path.'
						: pendingResolution === 'use-external'
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
