<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import type { CollaborationSession } from '@noura/editor';
	import { acquireNativeCollaboration } from '$lib/editor/native-collaboration';
	import type { CollaborationLease } from '$lib/editor/collaboration-registry';
	import CollaborativeTextSurface from '$lib/components/collaborative-text-surface.svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type {
		CoreEvent,
		RawMarkdownRead,
		UnmanagedFile,
		WorkspaceObject,
	} from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		file,
		onmanaged,
	}: {
		file: Pick<UnmanagedFile, 'relativePath' | 'title'> & {
			parseStatus?: UnmanagedFile['parseStatus'] | null;
		};
		onmanaged?: (object: WorkspaceObject) => void | Promise<void>;
	} = $props();

	type Conflict = { localBody: string; file: RawMarkdownRead };
	type Resolution = 'use-external' | 'replace-external';

	let collaboration = $state.raw<CollaborationSession | null>(null);
	let collaborationOpening = $state(true);
	let collaborationFailed = $state(false);
	let collaborationLease: CollaborationLease | null = null;
	let activationInProgress = $state(false);
	let activationNeedsReview = $state(false);
	let activationRequested: string[] | null = null;
	let editorDisposed = false;

	let loaded = $state.raw<RawMarkdownRead | null>(null);
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<string> | null>(null);
	let editorCleanup: (() => void) | null = null;
	let conflict = $state.raw<Conflict | null>(null);
	let error = $state<unknown | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);
	let message = $state<string | null>(null);

	function errorMessage(value: unknown) {
		if (value instanceof Error) return value.message;
		if (value && typeof value === 'object' && 'message' in value)
			return String(value.message);
		return 'Noura could not save this Markdown file.';
	}

	async function persist(body: string, generation: number) {
		if (activationInProgress || activationNeedsReview) return 'paused' as const;
		if (!loaded) return;
		const result = await getNouraClient().files.saveRawMarkdown({
			relativePath: loaded.relativePath,
			baseRevision: loaded.revision,
			baseBody: loaded.body,
			localBody: body,
		});
		if (activationInProgress || activationNeedsReview) return 'paused' as const;
		if (result.status === 'conflict') {
			conflict = { localBody: body, file: result.current };
			reviewOpen = true;
			coordinator?.pause();
			return 'paused' as const;
		}
		loaded = result.current;
		if (
			coordinator?.currentGeneration === generation &&
			result.current.body !== body
		) {
			editor?.setText(result.current.body);
			message = 'External changes merged';
		}
		if (result.managedObject) await onmanaged?.(result.managedObject);
	}

	function hasActivationDraft() {
		return Boolean(
			coordinator?.pendingEdits ||
			coordinator?.isWriting ||
			coordinator?.error ||
			(editor && loaded && editor.doc() !== loaded.body),
		);
	}
	async function activateCollaboration(objectIds: string[]) {
		if (collaboration || activationInProgress || activationNeedsReview) return;
		if (collaborationOpening) {
			activationRequested = objectIds;
			return;
		}
		activationInProgress = true;
		const retainedBody = hasActivationDraft()
			? (editor?.doc() ?? coordinator?.getDraft() ?? loaded?.body)
			: null;
		coordinator?.pause();
		let lease: CollaborationLease | null = null;
		try {
			lease = await acquireNativeCollaboration(file.relativePath);
			if (editorDisposed) {
				await lease?.release();
				lease = null;
				return;
			}
			if (!lease || !objectIds.includes(lease.session.bootstrap.objectId)) {
				await lease?.release();
				lease = null;
				coordinator?.resume();
				return;
			}
			if (retainedBody !== null && retainedBody !== undefined) {
				if (lease.session.text.toString() !== retainedBody) {
					lease.session.transact((text) => {
						text.delete(0, text.length);
						text.insert(0, retainedBody);
					});
					await lease.session.flush();
				}
				coordinator?.acceptDurable();
				if (loaded)
					loaded = {
						...loaded,
						body: retainedBody,
						revision: lease.session.revision,
					};
			}
			editorCleanup?.();
			editorCleanup = null;
			collaborationLease = lease;
			collaboration = lease.session;
			lease = null;
			error = null;
			collaborationFailed = false;
		} catch (value) {
			await lease?.release().catch(() => {});
			error = value;
			activationNeedsReview = true;
		} finally {
			activationInProgress = false;
		}
	}

	async function handleExternalEvent(event: CoreEvent) {
		if (event.type === 'collaboration:activated') {
			const payload = event.payload as { objectIds?: string[] };
			if (payload.objectIds) await activateCollaboration(payload.objectIds);
			return;
		}
		if (activationInProgress || activationNeedsReview) return;
		if (collaboration) return;
		if (
			!loaded ||
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			event.type !== 'file:changed'
		)
			return;
		const payload = event.payload as { paths?: string[] };
		if (!payload.paths?.includes(loaded.relativePath)) return;

		try {
			if (!coordinator?.pendingEdits && !coordinator?.isWriting) {
				const latest = await getNouraClient().files.readRawMarkdown({
					relativePath: loaded.relativePath,
				});
				loaded = latest;
				editor?.setText(latest.body);
				return;
			}

			const localBody = editor?.doc() ?? loaded.body;
			const result = await getNouraClient().files.reconcileRawMarkdown({
				relativePath: loaded.relativePath,
				baseRevision: loaded.revision,
				baseBody: loaded.body,
				localBody,
			});
			if (result.status === 'conflict') {
				conflict = { localBody, file: result.current };
				reviewOpen = true;
				coordinator?.pause();
				return;
			}
			loaded = result.current;
			if (result.status === 'merged') {
				editor?.setText(result.current.body);
				message = 'External changes merged';
				coordinator?.noteEdit(result.current.body);
			}
		} catch (value) {
			// Preserve the in-memory draft if the file was moved, deleted, or
			// became unreadable. Retry and Copy draft remain available below.
			error = value;
			coordinator?.pause();
		}
	}

	function connectEditor(handle: LiveMarkdownEditor | null) {
		editorCleanup?.();
		editorCleanup = null;
		editor = handle;
		if (!handle || !loaded) return;
		const localCoordinator = new AutosaveCoordinator<string>({
			write: persist,
			onStateChange: (state) => {
				error = state.error;
			},
		});
		coordinator = localCoordinator;
		const unregister = registerPendingDraft(
			`raw:${loaded.relativePath}`,
			() =>
				activationInProgress || activationNeedsReview
					? Promise.resolve(false)
					: localCoordinator.flush(),
			() =>
				activationInProgress ||
				activationNeedsReview ||
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

	function requestResolution(resolution: Resolution) {
		pendingResolution = resolution;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !loaded || !pendingResolution) return;
		resolving = true;
		try {
			const result = await getNouraClient().files.resolveRawConflict({
				relativePath: loaded.relativePath,
				currentRevision: conflict.file.revision,
				localBody: conflict.localBody,
				resolution: pendingResolution,
			});
			loaded = result.current;
			editor?.setText(result.current.body);
			coordinator?.acceptDurable();
			coordinator?.resume();
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			if (result.managedObject) await onmanaged?.(result.managedObject);
		} catch (value) {
			error = value;
		} finally {
			resolving = false;
		}
	}

	async function flushNow() {
		if (activationInProgress || activationNeedsReview) return;
		try {
			await collaboration?.flush();
			await coordinator?.flush();
		} catch (value) {
			error = value;
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
		void acquireNativeCollaboration(file.relativePath)
			.then(async (lease) => {
				if (disposed) {
					await lease?.release();
					return;
				}
				collaborationLease = lease;
				collaboration = lease?.session ?? null;
				if (!lease) {
					if (!/\.md$/i.test(file.relativePath))
						throw new Error(
							'This text file is not available for collaborative editing yet. You can still edit its workspace file in another application.',
						);
					const value = await getNouraClient().files.readRawMarkdown({
						relativePath: file.relativePath,
					});
					if (!disposed) loaded = value;
				}
				if (!disposed) {
					collaborationOpening = false;
					if (activationRequested && !collaboration) {
						const ids = activationRequested;
						activationRequested = null;
						void activateCollaboration(ids);
					}
				}
			})
			.catch((value) => {
				if (!disposed) {
					error = value;
					collaborationFailed = true;
					collaborationOpening = false;
				}
			});
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
			editorDisposed = true;
			unsubscribe?.();
			editorCleanup?.();
			void collaborationLease?.release().catch((value) => {
				error = value;
			});
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

<div class="flex min-h-0 flex-1 flex-col">
	<header class="flex min-h-16 items-center px-6">
		<div class="min-w-0">
			<h1 class="truncate text-base font-semibold">{file.title}</h1>
			<p class="truncate text-xs text-muted-foreground">{file.relativePath}</p>
		</div>
	</header>
	<Separator />
	{#if file.parseStatus === 'malformed'}
		<div class="px-6 pt-4">
			<Alert.Root
				><Warning /><Alert.Title>This file needs repair</Alert.Title
				><Alert.Description
					>Edit the complete Markdown source below. Noura will adopt its stable
					identity after the frontmatter becomes valid.</Alert.Description
				></Alert.Root
			>
		</div>
	{/if}
	{#if activationNeedsReview}
		<div class="px-6 pt-4">
			<Alert.Root>
				<Warning /><Alert.Title>Needs review</Alert.Title>
				<Alert.Description
					>Sharing became live while this editor had a draft. Autosave is
					paused. Your visible draft remains available to copy before reviewing
					the shared document.</Alert.Description
				>
				<Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() =>
							void navigator.clipboard.writeText(
								editor?.doc() ?? loaded?.body ?? '',
							)}>Copy draft</Button
					></Alert.Action
				>
			</Alert.Root>
		</div>
	{/if}
	{#if conflict}
		<div class="px-6 pt-4">
			<Alert.Root
				><Warning /><Alert.Title>This file changed in another app</Alert.Title
				><Alert.Description
					>Your draft is preserved while you review both versions.</Alert.Description
				><Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() => (reviewOpen = true)}>Review conflict</Button
					></Alert.Action
				></Alert.Root
			>
		</div>
	{/if}
	{#if error}
		<div class="px-6 pt-4">
			<Alert.Root variant="destructive"
				><Warning /><Alert.Title>Changes could not be saved</Alert.Title
				><Alert.Description>{errorMessage(error)}</Alert.Description
				><Alert.Action
					><div class="flex gap-2">
						<Button variant="outline" size="sm" onclick={() => void flushNow()}
							>Retry</Button
						><Button
							variant="outline"
							size="sm"
							onclick={() =>
								void navigator.clipboard.writeText(
									collaboration?.text.toString() ??
										editor?.doc() ??
										loaded?.body ??
										'',
								)}>Copy draft</Button
						>
					</div></Alert.Action
				></Alert.Root
			>
		</div>
	{/if}
	{#if message}<p class="px-6 pt-3 text-xs text-muted-foreground" role="status">
			{message}
		</p>{/if}
	{#if collaboration}
		<CollaborativeTextSurface
			session={collaboration}
			sourceRelativePath={file.relativePath}
			language={/\.md$/i.test(file.relativePath) ? 'markdown' : 'text'}
			onerror={(value) => (error = value)}
		/>
	{:else if loaded}
		<div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
			<LiveMarkdownSurface
				value={loaded.body}
				sourceRelativePath={loaded.relativePath}
				label="Raw Markdown document"
				onedit={(body) => coordinator?.noteEdit(body)}
				onready={connectEditor}
			/>
		</div>
	{:else if collaborationOpening && !collaborationFailed}
		<p class="p-6 text-sm text-muted-foreground">Opening document…</p>
	{/if}
</div>

{#if conflict}
	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl"
			><Sheet.Header
				><Sheet.Title>Review Markdown conflict</Sheet.Title><Sheet.Description
					>Noura preserved both versions.</Sheet.Description
				></Sheet.Header
			>
			<div
				class="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-5 overflow-y-auto px-4 pb-4"
			>
				<section>
					<h2 class="mb-3 text-sm font-medium">Your draft</h2>
					<MarkdownPreview markdown={conflict.localBody} />
				</section>
				<Separator orientation="vertical" />
				<section>
					<h2 class="mb-3 text-sm font-medium">File version</h2>
					<MarkdownPreview markdown={conflict.file.body} />
				</section>
			</div>
			<Sheet.Footer
				><Button variant="outline" onclick={() => (reviewOpen = false)}
					>Cancel and continue reviewing</Button
				><Button
					variant="outline"
					onclick={() => requestResolution('use-external')}
					>Use file version</Button
				><Button onclick={() => requestResolution('replace-external')}
					>Replace file with my version</Button
				></Sheet.Footer
			></Sheet.Content
		>
	</Sheet.Root>
{/if}

<AlertDialog.Root bind:open={confirmOpen}
	><AlertDialog.Content
		><AlertDialog.Header
			><AlertDialog.Title
				>{pendingResolution === 'use-external'
					? 'Use the file version?'
					: 'Replace the file version?'}</AlertDialog.Title
			><AlertDialog.Description
				>The displaced version is saved to recovery history first.</AlertDialog.Description
			></AlertDialog.Header
		><AlertDialog.Footer
			><AlertDialog.Cancel disabled={resolving}>Cancel</AlertDialog.Cancel
			><AlertDialog.Action
				disabled={resolving}
				onclick={() => void resolveConflict()}>Continue</AlertDialog.Action
			></AlertDialog.Footer
		></AlertDialog.Content
	></AlertDialog.Root
>
