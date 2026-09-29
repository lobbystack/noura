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
	import type {
		CoreEvent,
		RawMarkdownRead,
		UnmanagedFile,
		WorkspaceObject,
	} from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { DocumentSession, type TextBase } from '$lib/editor/document-session';
	import { saveErrorMessage } from '$lib/editor/messages';
	import {
		renameErrorMessage,
		renamedPath,
		splitFileName,
	} from '$lib/editor/rename';
	import { moveViewMemory } from '$lib/editor/view-memory';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import DocumentHeader from '$lib/components/document-header.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		file,
		onmanaged,
		onrenamed,
	}: {
		file: Pick<UnmanagedFile, 'relativePath' | 'title'> & {
			parseStatus?: UnmanagedFile['parseStatus'] | null;
		};
		onmanaged?: (object: WorkspaceObject) => void | Promise<void>;
		/** The file now lives at `relativePath`. */
		onrenamed?: (relativePath: string) => void | Promise<void>;
	} = $props();

	type Canonical = {
		read: RawMarkdownRead;
		managedObject: WorkspaceObject | null;
	};
	type Conflict = { localBody: string; file: RawMarkdownRead };
	type Resolution = 'use-external' | 'replace-external';

	const initialFile = () => file;
	let path = $state(initialFile().relativePath);
	const parseStatus = initialFile().parseStatus;
	let collaboration = $state.raw<CollaborationSession | null>(null);
	let collaborationOpening = $state(true);
	let collaborationFailed = $state(false);
	let collaborationLease: CollaborationLease | null = null;
	let activationInProgress = $state(false);
	let activationNeedsReview = $state(false);
	let activationRequested: string[] | null = null;
	let editorDisposed = false;

	let session = $state.raw<DocumentSession<Canonical> | null>(null);
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let conflict = $state.raw<Conflict | null>(null);
	let error = $state<unknown | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let pendingResolution = $state<Resolution | null>(null);
	let resolving = $state(false);
	let message = $state<string | null>(null);
	let clearMessageTimer: ReturnType<typeof setTimeout> | null = null;

	function showMessage(text: string) {
		message = text;
		if (clearMessageTimer) clearTimeout(clearMessageTimer);
		clearMessageTimer = setTimeout(() => {
			message = null;
			clearMessageTimer = null;
		}, 2400);
	}

	const baseOf = (read: RawMarkdownRead): TextBase => ({
		revision: read.revision,
		body: read.body,
	});

	function createSession(read: RawMarkdownRead) {
		return new DocumentSession<Canonical>({
			base: baseOf(read),
			blocked: () => activationInProgress || activationNeedsReview,
			save: async (base, body) => {
				const result = await getNouraClient().files.saveRawMarkdown({
					relativePath: path,
					baseRevision: base.revision,
					baseBody: base.body,
					localBody: body,
				});
				if (result.status === 'conflict')
					return {
						status: 'conflict',
						current: { read: result.current, managedObject: null },
					};
				return {
					status: 'saved',
					base: baseOf(result.current),
					canonical: {
						read: result.current,
						managedObject: result.managedObject,
					},
				};
			},
			read: async () => {
				const latest = await getNouraClient().files.readRawMarkdown({
					relativePath: path,
				});
				return {
					base: baseOf(latest),
					canonical: { read: latest, managedObject: null },
				};
			},
			onCanonical: async (canonical) => {
				error = null;
				if (canonical.managedObject) await onmanaged?.(canonical.managedObject);
			},
			onConflict: ({ localBody, current }) => {
				conflict = { localBody, file: current.read };
				reviewOpen = true;
			},
			onMerged: () => showMessage('Merged changes from another app'),
			onStateChange: (state) => {
				error = state.error;
			},
		});
	}

	function currentText() {
		return collaboration?.text.toString() ?? session?.text() ?? '';
	}

	async function activateCollaboration(objectIds: string[]) {
		if (collaboration || activationInProgress || activationNeedsReview) return;
		if (collaborationOpening) {
			activationRequested = objectIds;
			return;
		}
		activationInProgress = true;
		const retainedBody = session?.hasPendingWork ? session.text() : null;
		session?.autosave.pause();
		let lease: CollaborationLease | null = null;
		try {
			lease = await acquireNativeCollaboration(path);
			if (editorDisposed) {
				await lease?.release();
				lease = null;
				return;
			}
			if (!lease || !objectIds.includes(lease.session.bootstrap.objectId)) {
				await lease?.release();
				lease = null;
				session?.autosave.resume();
				return;
			}
			if (
				retainedBody !== null &&
				lease.session.text.toString() !== retainedBody
			) {
				lease.session.transact((text) => {
					text.delete(0, text.length);
					text.insert(0, retainedBody);
				});
				await lease.session.flush();
			}
			session?.discard();
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
		if (activationInProgress || activationNeedsReview || collaboration) return;
		if (
			!session ||
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			event.type !== 'file:changed'
		)
			return;
		const payload = event.payload as { paths?: string[] };
		if (!payload.paths?.includes(path)) return;
		try {
			await session.externalChange();
		} catch (value) {
			// The file was moved, deleted, or became unreadable. The text stays
			// in the editor; Try again and Copy text remain available.
			error = value;
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

	async function rename(name: string) {
		const from = path;
		const to = renamedPath(from, name);
		if (!to) return;
		const move = () => getNouraClient().files.move({ from, to });
		try {
			if (session) await session.exclusive(move);
			else await move();
		} catch (value) {
			toast.error(renameErrorMessage(value));
			throw value;
		}
		path = to;
		moveViewMemory(`file:${from}`, `file:${to}`);
		await onrenamed?.(to);
	}

	function requestResolution(resolution: Resolution) {
		pendingResolution = resolution;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !session || !pendingResolution) return;
		resolving = true;
		try {
			const resolution = pendingResolution;
			const result = await getNouraClient().files.resolveRawConflict({
				relativePath: path,
				currentRevision: conflict.file.revision,
				localBody: conflict.localBody,
				resolution,
			});
			session.resolved(baseOf(result.current), resolution === 'use-external');
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
			showMessage(
				resolution === 'use-external'
					? 'Switched to the file’s version'
					: 'Saved your version',
			);
			if (result.managedObject) await onmanaged?.(result.managedObject);
		} catch (value) {
			error = value;
		} finally {
			resolving = false;
		}
	}

	async function flushNow() {
		if (activationInProgress || activationNeedsReview) return false;
		try {
			await collaboration?.flush();
			return (await session?.flush()) ?? true;
		} catch (value) {
			error = value;
			return false;
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
		const unregister = registerPendingDraft(
			`raw:${initialFile().relativePath}`,
			() => flushNow(),
			() =>
				activationInProgress ||
				activationNeedsReview ||
				(session?.hasPendingWork ?? false),
			{
				label: () => splitFileName(path).stem,
				problem: () =>
					conflict
						? 'The file changed in another app.'
						: error
							? saveErrorMessage(error)
							: null,
				discard: () => {
					session?.discard();
					activationNeedsReview = false;
					conflict = null;
				},
			},
		);
		// Read the file while the collaboration check runs instead of after it.
		const read = /\.md$/i.test(path)
			? getNouraClient().files.readRawMarkdown({ relativePath: path })
			: null;
		read?.catch(() => {});
		void openNativeCollaboration(path)
			.then(async (lease) => {
				if (disposed) {
					await lease?.release();
					return;
				}
				collaborationLease = lease;
				collaboration = lease?.session ?? null;
				if (!lease) {
					if (!read)
						throw new Error(
							'noura can only edit this file while it’s shared. You can still open it in another app.',
						);
					const value = await read;
					if (!disposed) session = createSession(value);
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
			void collaborationLease?.release().catch((value) => {
				error = value;
			});
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

<div class="flex min-h-0 flex-1 flex-col">
	<DocumentHeader
		relativePath={path}
		disabled={collaborationOpening || activationInProgress}
		onrename={rename}
		ondone={() => editor?.focus()}
	/>
	<Separator />
	{#if parseStatus === 'malformed'}
		<div class="px-6 pt-4">
			<Alert.Root
				><Warning /><Alert.Title
					>The properties at the top of this file have an error</Alert.Title
				><Alert.Description
					>Fix them in the text below. Everything else works as usual.</Alert.Description
				></Alert.Root
			>
		</div>
	{/if}
	{#if activationNeedsReview}
		<div class="px-6 pt-4">
			<Alert.Root>
				<Warning /><Alert.Title
					>Sharing started while you had unsaved changes</Alert.Title
				>
				<Alert.Description
					>Autosave is paused so your changes don’t overwrite the shared
					version. Copy your text, then reopen the file.</Alert.Description
				>
				<Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() => void navigator.clipboard.writeText(currentText())}
						>Copy text</Button
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
					>Your text is still here. Compare both versions and choose one.</Alert.Description
				><Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() => (reviewOpen = true)}>Compare</Button
					></Alert.Action
				></Alert.Root
			>
		</div>
	{/if}
	{#if error}
		<div class="px-6 pt-4">
			<Alert.Root variant="destructive"
				><Warning /><Alert.Title>Changes aren’t saved</Alert.Title
				><Alert.Description>{saveErrorMessage(error)}</Alert.Description
				><Alert.Action
					><div class="flex gap-2">
						<Button variant="outline" size="sm" onclick={() => void flushNow()}
							>Try again</Button
						><Button
							variant="outline"
							size="sm"
							onclick={() => void navigator.clipboard.writeText(currentText())}
							>Copy text</Button
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
			sourceRelativePath={path}
			language={/\.md$/i.test(path) ? 'markdown' : 'text'}
			onerror={(value) => (error = value)}
		/>
	{:else if session}
		<div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
			<LiveMarkdownSurface
				value={session.base.body}
				sourceRelativePath={path}
				label="File text"
				memoryKey={`file:${path}`}
				autofocus
				onchange={() => session?.edited()}
				onready={connectEditor}
			/>
		</div>
	{:else if collaborationOpening && !collaborationFailed}
		<!-- Fast opens finish before this fades in, so they never flash it. -->
		<p
			class="p-6 text-sm text-muted-foreground animate-in fade-in fill-mode-backwards delay-300"
			role="status"
		>
			Opening…
		</p>
	{/if}
</div>

{#if conflict}
	<Sheet.Root bind:open={reviewOpen}>
		<Sheet.Content class="sm:max-w-2xl"
			><Sheet.Header
				><Sheet.Title>Choose a version</Sheet.Title><Sheet.Description
					>Both versions are kept until you choose.</Sheet.Description
				></Sheet.Header
			>
			<div
				class="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-5 overflow-y-auto px-4 pb-4"
			>
				<section class="min-w-0">
					<h2 class="mb-3 text-sm font-medium">Your version</h2>
					<MarkdownPreview markdown={conflict.localBody} />
				</section>
				<Separator orientation="vertical" />
				<section class="min-w-0">
					<h2 class="mb-3 text-sm font-medium">The file’s version</h2>
					<MarkdownPreview markdown={conflict.file.body} />
				</section>
			</div>
			<Sheet.Footer
				><Button variant="outline" onclick={() => (reviewOpen = false)}
					>Keep editing</Button
				><Button
					variant="outline"
					onclick={() => requestResolution('use-external')}
					>Use the file’s version</Button
				><Button onclick={() => requestResolution('replace-external')}
					>Use my version</Button
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
					? 'Use the file’s version?'
					: 'Use your version?'}</AlertDialog.Title
			><AlertDialog.Description
				>{pendingResolution === 'use-external'
					? 'Your version is kept as a backup first.'
					: 'The file’s version is kept as a backup first.'}</AlertDialog.Description
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
