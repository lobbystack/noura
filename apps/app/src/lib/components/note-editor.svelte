<script lang="ts">
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import type { CollaborationSession } from '@noura/editor';
	import {
		acquireNativeCollaboration,
		openNativeCollaboration,
	} from '$lib/editor/native-collaboration';
	import type { CollaborationLease } from '$lib/editor/collaboration-registry';
	import CollaborativeTextSurface from '$lib/components/collaborative-text-surface.svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type { CoreEvent, Note } from '@noura/workspace';
	import { isCoreError } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import {
		saveNoteWithReconciliation,
		type NoteDraft,
	} from '$lib/editor/note-save';
	import { DocumentSession, type TextBase } from '$lib/editor/document-session';
	import { saveErrorMessage } from '$lib/editor/messages';
	import {
		renameErrorMessage,
		renamedPath,
		splitFileName,
	} from '$lib/editor/rename';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import DocumentHeader from '$lib/components/document-header.svelte';
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
		/** A new version of the note is on disk (saved, merged, or renamed). */
		onsaved?: (updated: Note) => void;
		/** Freshly created notes start with the name selected for typing. */
		autofocusTitle?: boolean;
	} = $props();

	type ConflictState = { draft: NoteDraft; file: Note; deleted?: boolean };
	type Resolution = 'use-external' | 'replace-external';

	const initialNote = () => note;
	let currentNote = $state.raw<Note | null>(initialNote());
	let collaboration = $state.raw<CollaborationSession | null>(null);
	let collaborationOpening = $state(true);
	let collaborationFailed = $state(false);
	let collaborationLease: CollaborationLease | null = null;
	let activationInProgress = $state(false);
	let activationNeedsReview = $state(false);
	let activationRequested = false;
	let editorDisposed = false;

	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let autosaveError = $state<unknown | null>(null);
	let conflict = $state.raw<ConflictState | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);
	let transientMessage = $state<string | null>(null);
	let clearMessageTimer: ReturnType<typeof setTimeout> | null = null;

	const fileName = () =>
		currentNote ? splitFileName(currentNote.relativePath).stem : 'Untitled';

	function showMessage(message: string) {
		transientMessage = message;
		if (clearMessageTimer) clearTimeout(clearMessageTimer);
		clearMessageTimer = setTimeout(() => {
			transientMessage = null;
			clearMessageTimer = null;
		}, 2400);
	}

	function adoptNote(value: Note) {
		currentNote = value;
		onsaved?.(value);
	}

	const baseOf = (value: Note): TextBase => ({
		revision: value.revision,
		body: value.body,
	});

	/** Autosave for the local editor; collaboration saves on its own. */
	const session = initialNote()
		? new DocumentSession<Note>({
				base: baseOf(initialNote()!),
				blocked: () => activationInProgress || activationNeedsReview,
				save: async (base, body) => {
					const target = currentNote!;
					const result = await saveNoteWithReconciliation(
						target,
						{ revision: base.revision, title: target.title, body: base.body },
						{ title: target.title, body },
					);
					if (result.status === 'conflict')
						return { status: 'conflict', current: result.current };
					return {
						status: 'saved',
						base: baseOf(result.value),
						canonical: result.value,
					};
				},
				read: async () => {
					const latest = await getNouraClient().notes.get(currentNote!.id);
					return { base: baseOf(latest), canonical: latest };
				},
				onCanonical: adoptNote,
				onConflict: ({ localBody, current }) =>
					openConflict({ title: fileName(), body: localBody }, current),
				onMerged: () => showMessage('Merged changes from another app'),
				onStateChange: (state) => {
					autosaveError = state.error;
				},
			})
		: null;

	function currentBody() {
		return collaboration?.text.toString() ?? session?.text() ?? '';
	}

	async function flushNow() {
		if (activationInProgress || activationNeedsReview) return false;
		try {
			await collaboration?.flush();
			return (await session?.flush()) ?? !collaborationFailed;
		} catch (error) {
			autosaveError = error;
			return false;
		}
	}

	function connectEditor(handle: LiveMarkdownEditor | null) {
		editor = handle;
		session?.attach(
			handle
				? {
						text: () => handle.doc(),
						setText: (text) => handle.setText(text),
						rebase: (base, target) => handle.rebase(base, target),
					}
				: null,
		);
	}

	function openConflict(draft: NoteDraft, file: Note, deleted = false) {
		conflict = { draft, file, deleted };
		reviewOpen = true;
		session?.autosave.pause();
	}

	async function activateCollaboration() {
		if (
			collaboration ||
			activationInProgress ||
			activationNeedsReview ||
			!currentNote
		)
			return;
		if (collaborationOpening) {
			activationRequested = true;
			return;
		}
		activationInProgress = true;
		const retainedBody = session?.hasPendingWork ? session.text() : null;
		session?.autosave.pause();
		let lease: CollaborationLease | null = null;
		try {
			const target = currentNote;
			lease = await acquireNativeCollaboration(target.relativePath);
			if (editorDisposed) {
				await lease?.release();
				lease = null;
				return;
			}
			if (!lease) throw new Error('Sharing isn’t ready yet. Reopen this note.');
			if (retainedBody !== null) {
				const result = await getNouraClient().notes.update(target.id, {
					expectedRevision: lease.session.revision,
					body: retainedBody,
				});
				await lease.release();
				lease = null;
				lease = await acquireNativeCollaboration(result.value.relativePath);
				if (!lease)
					throw new Error(
						'Your changes were saved, but sharing couldn’t start. Reopen this note.',
					);
				if (editorDisposed) {
					await lease.release();
					lease = null;
					return;
				}
				adoptNote(result.value as Note);
			}
			session?.discard();
			collaborationLease = lease;
			collaboration = lease.session;
			lease = null;
			autosaveError = null;
			collaborationFailed = false;
		} catch (error) {
			await lease?.release().catch(() => {});
			autosaveError = error;
			activationNeedsReview = true;
		} finally {
			activationInProgress = false;
		}
	}

	async function handleExternalEvent(event: CoreEvent) {
		const target = currentNote;
		if (event.type === 'collaboration:activated') {
			const payload = event.payload as { objectIds?: string[] };
			if (target && payload.objectIds?.includes(target.id))
				await activateCollaboration();
			return;
		}
		if (activationInProgress || activationNeedsReview) return;
		if (
			!target ||
			(!collaboration &&
				event.source !== 'external' &&
				event.source !== 'reconciliation') ||
			!['object:updated', 'object:moved', 'object:deleted'].includes(event.type)
		)
			return;
		const payload = event.payload as { id?: string };
		if (payload.id !== target.id) return;
		if (collaboration) {
			if (event.type === 'object:deleted') {
				autosaveError = new Error(
					'This note’s file was deleted. Your shared text is still here.',
				);
				return;
			}
			try {
				adoptNote(await getNouraClient().notes.get(target.id));
			} catch (error) {
				autosaveError = error;
			}
			return;
		}
		if (event.type === 'object:deleted') {
			openConflict({ title: fileName(), body: currentBody() }, target, true);
			return;
		}
		try {
			await session?.externalChange();
		} catch (error) {
			if (isCoreError(error) && error.code === 'object_not_found') {
				// The file disappeared between the event and the read; show the
				// same review surface the deleted event would.
				openConflict({ title: fileName(), body: currentBody() }, target, true);
				return;
			}
			autosaveError = error;
		}
	}

	async function rename(name: string) {
		const target = currentNote;
		if (!target) return;
		const to = renamedPath(target.relativePath, name);
		if (!to) return;
		const client = getNouraClient();
		const move = async () => {
			await client.files.move({ from: target.relativePath, to });
			// Notes keep a title in their properties; keep it in step.
			let latest = await client.notes.get(target.id);
			if (latest.title !== name) {
				latest = (
					await client.notes.update(target.id, {
						expectedRevision: latest.revision,
						title: name,
					})
				).value as Note;
			}
			return latest;
		};
		try {
			const latest = session ? await session.exclusive(move) : await move();
			session?.adoptRevision(baseOf(latest));
			adoptNote(latest);
		} catch (error) {
			toast.error(renameErrorMessage(error));
			throw error;
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
			const resolution = pendingResolution;
			const resolved = (await getNouraClient().notes.resolveManagedConflict({
				id: currentNote.id,
				currentRevision: pendingConflict.file.revision,
				relativePath: currentNote.relativePath,
				created: currentNote.created,
				localTitle: currentNote.title,
				localBody: pendingConflict.draft.body,
				localProperties: currentNote.properties,
				resolution,
			})) as Note;
			adoptNote(resolved);
			session?.resolved(baseOf(resolved), resolution === 'use-external');
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			showMessage(
				pendingConflict.deleted
					? 'File restored'
					: resolution === 'use-external'
						? 'Switched to the file’s version'
						: 'Saved your version',
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
		const unregister = currentNote
			? registerPendingDraft(
					currentNote.id,
					() => flushNow(),
					() =>
						activationInProgress ||
						activationNeedsReview ||
						(session?.hasPendingWork ?? false),
					{
						label: fileName,
						problem: () =>
							conflict
								? 'The file changed in another app.'
								: autosaveError
									? saveErrorMessage(autosaveError)
									: null,
						discard: () => {
							session?.discard();
							activationNeedsReview = false;
							conflict = null;
						},
					},
				)
			: () => {};
		if (currentNote) {
			void openNativeCollaboration(currentNote.relativePath)
				.then(async (lease) => {
					if (disposed) {
						await lease?.release();
						return;
					}
					collaborationLease = lease;
					collaboration = lease?.session ?? null;
					if (collaboration) session?.discard();
					collaborationOpening = false;
					if (activationRequested && !collaboration) {
						activationRequested = false;
						void activateCollaboration();
					}
				})
				.catch((error) => {
					if (!disposed) {
						collaborationOpening = false;
						collaborationFailed = true;
						autosaveError = error;
					}
				});
		} else collaborationOpening = false;
		void getNouraClient()
			.events.subscribe((event) => void handleExternalEvent(event))
			.then((unlisten) => {
				if (disposed) unlisten();
				else unsubscribe = unlisten;
			});
		return () => {
			disposed = true;
			editorDisposed = true;
			unsubscribe?.();
			unregister();
			if (clearMessageTimer) clearTimeout(clearMessageTimer);
			session?.destroy();
			void collaborationLease?.release().catch((error) => {
				autosaveError = error;
			});
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

{#if !currentNote}
	<EmptyState
		icon={NotePencil}
		title="No file open"
		description="Pick a file from the sidebar, or create a new note."
	/>
{:else}
	<div class="flex min-h-0 flex-1 flex-col">
		<DocumentHeader
			relativePath={currentNote.relativePath}
			autofocus={autofocusTitle}
			disabled={collaborationOpening ||
				activationInProgress ||
				collaboration?.bootstrap.readOnly}
			onrename={rename}
			ondone={() => editor?.focus()}
		/>
		<Separator />

		{#if activationNeedsReview}
			<div class="px-6 pt-4">
				<Alert.Root>
					<Warning /><Alert.Title
						>Sharing started while you had unsaved changes</Alert.Title
					>
					<Alert.Description
						>Autosave is paused so your changes don’t overwrite the shared
						version. Copy your text, then reopen the note.</Alert.Description
					>
					<Alert.Action
						><Button
							variant="outline"
							size="sm"
							onclick={() => void navigator.clipboard.writeText(currentBody())}
							>Copy text</Button
						></Alert.Action
					>
				</Alert.Root>
			</div>
		{/if}
		{#if conflict}
			<div class="px-6 pt-4">
				<Alert.Root>
					<Warning />
					<Alert.Title
						>{conflict.deleted
							? 'This file was deleted'
							: 'This file changed in another app'}</Alert.Title
					>
					<Alert.Description>
						{conflict.deleted
							? 'Your text is still here. You can put the file back.'
							: 'Your text is still here. Compare both versions and choose one.'}
					</Alert.Description>
					<Alert.Action>
						<Button
							variant="outline"
							size="sm"
							onclick={() => (reviewOpen = true)}
						>
							{conflict.deleted ? 'Restore' : 'Compare'}
						</Button>
					</Alert.Action>
				</Alert.Root>
			</div>
		{/if}

		{#if autosaveError}
			<div class="px-6 pt-4">
				<Alert.Root variant="destructive">
					<Warning />
					<Alert.Title>Changes aren’t saved</Alert.Title>
					<Alert.Description
						>{saveErrorMessage(autosaveError)}</Alert.Description
					>
					<Alert.Action>
						<Button variant="outline" size="sm" onclick={() => void flushNow()}>
							Try again
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
			{#if collaboration}
				<CollaborativeTextSurface
					session={collaboration}
					sourceRelativePath={currentNote.relativePath}
					language="markdown"
					onerror={(error) => (autosaveError = error)}
				/>
			{:else if collaborationOpening}
				<!-- Fast opens finish before this fades in, so they never flash it. -->
				<p
					class="p-6 text-sm text-muted-foreground animate-in fade-in fill-mode-backwards delay-300"
					role="status"
				>
					Opening…
				</p>
			{:else if !collaborationFailed && session}
				<LiveMarkdownSurface
					value={session.base.body}
					sourceRelativePath={currentNote.relativePath}
					label="Note text"
					memoryKey={`note:${currentNote.id}`}
					autofocus={!autofocusTitle}
					onchange={() => session.edited()}
					onready={connectEditor}
				/>
			{/if}
		</div>
	</div>

	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl">
			<Sheet.Header>
				<Sheet.Title
					>{conflict?.deleted
						? 'Restore the file'
						: 'Choose a version'}</Sheet.Title
				>
				<Sheet.Description>
					{conflict?.deleted
						? 'The file was deleted outside noura. Your text can put it back.'
						: 'Both versions are kept until you choose.'}
				</Sheet.Description>
			</Sheet.Header>
			{#if conflict}
				<div
					class="grid min-h-0 flex-1 gap-5 overflow-y-auto px-4 pb-4 {conflict.deleted
						? 'grid-cols-1'
						: 'grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]'}"
				>
					<section class="min-w-0">
						<h2 class="mb-3 text-sm font-medium">Your version</h2>
						<MarkdownPreview markdown={conflict.draft.body} />
					</section>
					{#if !conflict.deleted}
						<Separator orientation="vertical" />
						<section class="min-w-0">
							<h2 class="mb-3 text-sm font-medium">The file’s version</h2>
							<MarkdownPreview markdown={conflict.file.body} />
						</section>
					{/if}
				</div>
			{/if}
			<Sheet.Footer>
				<Button variant="outline" onclick={() => (reviewOpen = false)}>
					Keep editing
				</Button>
				{#if !conflict?.deleted}
					<Button
						variant="outline"
						onclick={() => requestResolution('use-external')}
					>
						Use the file’s version
					</Button>
				{/if}
				<Button onclick={() => requestResolution('replace-external')}>
					{conflict?.deleted ? 'Restore file' : 'Use my version'}
				</Button>
			</Sheet.Footer>
		</Sheet.Content>
	</Sheet.Root>

	<AlertDialog.Root bind:open={confirmOpen}>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>
					{conflict?.deleted
						? 'Restore the file?'
						: pendingResolution === 'use-external'
							? 'Use the file’s version?'
							: 'Use your version?'}
				</AlertDialog.Title>
				<AlertDialog.Description>
					{conflict?.deleted
						? 'Your text is written back to the same place.'
						: pendingResolution === 'use-external'
							? 'Your version is kept as a backup first.'
							: 'The file’s version is kept as a backup first.'}
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
