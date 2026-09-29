<script lang="ts">
	import { editorObjectEvent } from '$lib/object-events';
	import { onMount } from 'svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type { CoreEvent, Project } from '@noura/workspace';
	import { isCoreError } from '@noura/workspace';
	import { PROJECT_STATUSES } from '@noura/shared';
	import { choiceLabel } from '$lib/property-labels';
	import { getNouraClient } from '$lib/state.svelte';
	import {
		ManagedDraftSession,
		rebaseManagedDraft,
		type ManagedSaveResult,
	} from '$lib/editor/managed-draft-session';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import Warning from 'phosphor-svelte/lib/Warning';

	let {
		project,
		onupdated,
	}: { project: Project; onupdated?: (project: Project) => void } = $props();
	type Draft = {
		title: string;
		body: string;
		properties: Record<string, unknown>;
	};
	type Conflict = { draft: Draft; file: Project; deleted?: boolean };
	const statuses = PROJECT_STATUSES;
	const initialProject = () => project;
	let current = $state.raw(initialProject());
	let title = $state(initialProject().title);
	let displayBody = $state(initialProject().body);
	let properties = $state<Record<string, unknown>>({
		...initialProject().properties,
	});
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let error = $state<unknown | null>(null);
	let conflict = $state.raw<Conflict | null>(null);

	function draft(): Draft {
		return {
			title,
			body: editor?.doc() ?? displayBody,
			properties: { ...properties },
		};
	}

	// Saves and reloads move the form to a new version as a rebase, so edits
	// made while either is in flight are kept.
	const session = new ManagedDraftSession<Project>({
		base: initialProject(),
		// The native side returns the saved project as a plain object.
		save: async (input) =>
			(await getNouraClient().projects.saveDraft(
				input,
			)) as ManagedSaveResult<Project>,
		read: () => getNouraClient().projects.get(current.id),
		onCanonical: (value) => {
			current = value;
			onupdated?.(value);
		},
		onConflict: ({ draft: local, file }) => {
			conflict = { draft: local, file };
		},
		onStateChange: (state) => (error = state.error),
	});
	session.attach({
		read: draft,
		rebase: (from, to) => {
			const next = rebaseManagedDraft(draft(), from, to);
			title = next.title;
			properties = next.properties;
			if (from.body === to.body) return;
			if (editor) {
				editor.rebase(from.body, to.body);
				displayBody = editor.doc();
			} else displayBody = to.body;
		},
	});

	function showDeleted() {
		conflict = { draft: draft(), file: current, deleted: true };
		session.pause();
	}

	async function handleExternalEvent(event: CoreEvent) {
		// A bulk external change carries this project among many others.
		const own = editorObjectEvent(event, current.id);
		if (!own || conflict) return;
		if (own.type === 'object:deleted') {
			showDeleted();
			return;
		}
		try {
			await session.externalChange();
		} catch (value) {
			if (isCoreError(value) && value.code === 'object_not_found') {
				// The file disappeared between the event and the read; show the
				// same review surface the deleted event would.
				showDeleted();
				return;
			}
			error = value;
		}
	}
	function connectEditor(handle: LiveMarkdownEditor | null) {
		editor = handle;
	}
	function editStatus(value: string) {
		properties = { ...properties, status: value };
		session.edited();
		void session.flush();
	}
	async function resolveConflict(
		resolution: 'use-external' | 'replace-external',
	) {
		const pending = conflict;
		if (!pending || (pending.deleted && resolution === 'use-external')) return;
		try {
			const value = (await getNouraClient().projects.resolveManagedConflict({
				id: current.id,
				currentRevision: pending.file.revision,
				relativePath: current.relativePath,
				created: current.created,
				localTitle: pending.draft.title,
				localBody: pending.draft.body,
				localProperties: pending.draft.properties,
				resolution,
			})) as Project;
			conflict = null;
			error = null;
			session.resolved(value, true);
		} catch (value) {
			error = value;
		}
	}
	function handleShortcut(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
			event.preventDefault();
			void session.flush();
		}
	}
	onMount(() => {
		let disposed = false;
		let unsubscribe: (() => void) | undefined;
		const unregister = registerPendingDraft(
			current.id,
			() => session.flush(),
			() => session.hasPendingWork,
		);
		void getNouraClient()
			.events.subscribe((event) => void handleExternalEvent(event))
			.then((unlisten) => {
				if (disposed) unlisten();
				else unsubscribe = unlisten;
			});
		return () => {
			disposed = true;
			unsubscribe?.();
			unregister();
			session.destroy();
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

<div class="flex min-h-0 flex-1 flex-col">
	<header class="flex min-h-16 items-center gap-3 px-6">
		<input
			bind:value={title}
			oninput={() => session.edited()}
			onblur={() => void session.flush()}
			aria-label="Project title"
			class="min-w-0 flex-1 bg-transparent text-base font-semibold outline-none"
		/><Select.Root
			type="single"
			value={String(properties.status ?? 'planned')}
			onValueChange={(value) => value && editStatus(value)}
			><Select.Trigger size="sm" class="w-36" aria-label="Project status"
				>{choiceLabel(String(properties.status ?? 'planned'))}</Select.Trigger
			><Select.Content
				><Select.Group
					>{#each statuses as status (status)}<Select.Item
							value={status}
							label={choiceLabel(status)}>{choiceLabel(status)}</Select.Item
						>{/each}</Select.Group
				></Select.Content
			></Select.Root
		>
	</header>
	<Separator />
	{#if conflict}<div class="px-6 pt-4">
			<Alert.Root
				><Warning /><Alert.Title
					>{conflict.deleted
						? 'This project file was deleted'
						: 'This project changed in another app'}</Alert.Title
				><Alert.Description
					>{conflict.deleted
						? 'Your draft is still available and can restore the file.'
						: 'Choose which version should remain after reviewing the file.'}</Alert.Description
				><Alert.Action
					><div class="flex gap-2">
						{#if !conflict.deleted}<Button
								variant="outline"
								size="sm"
								onclick={() => void resolveConflict('use-external')}
								>Use file version</Button
							>{/if}<Button
							size="sm"
							onclick={() => void resolveConflict('replace-external')}
							>{conflict.deleted
								? 'Restore project file'
								: 'Replace file'}</Button
						>
					</div></Alert.Action
				></Alert.Root
			>
		</div>{/if}
	{#if error}<div class="px-6 pt-4">
			<Alert.Root variant="destructive"
				><Warning /><Alert.Title>Changes could not be saved</Alert.Title
				><Alert.Description
					>Retry after checking workspace access.</Alert.Description
				><Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() => void session.flush()}>Retry</Button
					></Alert.Action
				></Alert.Root
			>
		</div>{/if}
	<div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
		<LiveMarkdownSurface
			value={displayBody}
			sourceRelativePath={current.relativePath}
			label="Project overview"
			onedit={() => session.edited()}
			onready={connectEditor}
		/>
	</div>
</div>
