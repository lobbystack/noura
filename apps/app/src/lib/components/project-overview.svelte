<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type { CoreEvent, Project } from '@noura/workspace';
	import { isCoreError } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
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
	const statuses = [
		'planned',
		'active',
		'on-hold',
		'completed',
		'cancelled',
	] as const;
	const initialProject = () => project;
	let current = $state.raw(initialProject());
	let title = $state(initialProject().title);
	let body = $state(initialProject().body);
	let displayBody = $state(initialProject().body);
	let revision = $state(initialProject().revision);
	let baseTitle = $state(initialProject().title);
	let properties = $state<Record<string, unknown>>({
		...initialProject().properties,
	});
	let baseProperties = $state<Record<string, unknown>>({
		...initialProject().properties,
	});
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<Draft> | null>(null);
	let cleanup: (() => void) | null = null;
	let error = $state<unknown | null>(null);
	let conflict = $state.raw<Conflict | null>(null);

	function draft(): Draft {
		return {
			title,
			body: editor?.doc() ?? displayBody,
			properties: { ...properties },
		};
	}
	function input(value: Draft) {
		return {
			id: current.id,
			baseRevision: revision,
			baseTitle,
			baseBody: body,
			baseProperties,
			localTitle: value.title,
			localBody: value.body,
			localProperties: value.properties,
		};
	}
	function adoptCanonical(value: Project) {
		current = value;
		revision = value.revision;
		baseTitle = value.title;
		body = value.body;
		baseProperties = { ...value.properties };
		onupdated?.(value);
	}
	function syncDraft(value: Draft) {
		title = value.title;
		displayBody = value.body;
		properties = { ...value.properties };
		editor?.setText(value.body);
	}
	async function persist(value: Draft, generation: number) {
		const result = await getNouraClient().projects.saveDraft(input(value));
		if (result.status === 'conflict') {
			conflict = { draft: value, file: result.current as Project };
			coordinator?.pause();
			return 'paused' as const;
		}
		const canonical = result.current as Project;
		adoptCanonical(canonical);
		if (coordinator?.currentGeneration === generation) {
			syncDraft(
				result.status === 'merged'
					? {
							title: result.title,
							body: result.body,
							properties: result.properties,
						}
					: canonical,
			);
		}
	}
	async function handleExternalEvent(event: CoreEvent) {
		if (
			(event.source !== 'external' && event.source !== 'reconciliation') ||
			!['object:updated', 'object:moved', 'object:deleted'].includes(event.type)
		)
			return;
		const payload = event.payload as { id?: string };
		if (payload.id !== current.id) return;
		if (event.type === 'object:deleted') {
			conflict = { draft: draft(), file: current, deleted: true };
			coordinator?.pause();
			return;
		}

		try {
			const latest = await getNouraClient().projects.get(current.id);
			if (!coordinator?.pendingEdits && !coordinator?.isWriting) {
				adoptCanonical(latest);
				syncDraft(latest);
				return;
			}

			const local = draft();
			const result = await getNouraClient().projects.reconcileManaged(
				input(local),
			);
			if (result.status === 'conflict') {
				conflict = { draft: local, file: result.current as Project };
				coordinator?.pause();
				return;
			}
			adoptCanonical(result.current as Project);
			if (result.status === 'merged') {
				syncDraft({
					title: result.title,
					body: result.body,
					properties: result.properties,
				});
			}
			coordinator?.noteEdit(draft());
		} catch (value) {
			if (isCoreError(value) && value.code === 'object_not_found') {
				// The file disappeared between the event and the fetch; show the
				// same review surface the deleted event would.
				conflict = { draft: draft(), file: current, deleted: true };
				coordinator?.pause();
				return;
			}
			error = value;
		}
	}
	function connectEditor(handle: LiveMarkdownEditor | null) {
		cleanup?.();
		cleanup = null;
		editor = handle;
		if (!handle) return;
		const local = new AutosaveCoordinator<Draft>({
			write: persist,
			onStateChange: (state) => (error = state.error),
		});
		coordinator = local;
		const unregister = registerPendingDraft(
			current.id,
			() => local.flush(),
			() => local.pendingEdits > 0 || local.isWriting || local.error !== null,
		);
		cleanup = () => {
			unregister();
			local.destroy();
			if (coordinator === local) coordinator = null;
		};
	}
	function editStatus(value: string) {
		properties = { ...properties, status: value };
		coordinator?.noteEdit(draft());
		void coordinator?.flush();
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
			adoptCanonical(value);
			syncDraft(value);
			conflict = null;
			error = null;
			coordinator?.acceptDurable();
			coordinator?.resume();
		} catch (value) {
			error = value;
		}
	}
	function handleShortcut(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
			event.preventDefault();
			void coordinator?.flush();
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
			cleanup?.();
		};
	});
</script>

<svelte:window onkeydown={handleShortcut} />

<div class="flex min-h-0 flex-1 flex-col">
	<header class="flex min-h-16 items-center gap-3 px-6">
		<input
			bind:value={title}
			oninput={() => coordinator?.noteEdit(draft())}
			onblur={() => void coordinator?.flush()}
			aria-label="Project title"
			class="min-w-0 flex-1 bg-transparent text-base font-semibold outline-none"
		/><Select.Root
			type="single"
			value={String(properties.status ?? 'planned')}
			onValueChange={(value) => value && editStatus(value)}
			><Select.Trigger size="sm" class="w-36" aria-label="Project status"
				>{String(properties.status ?? 'planned')}</Select.Trigger
			><Select.Content
				><Select.Group
					>{#each statuses as status (status)}<Select.Item
							value={status}
							label={status}>{status}</Select.Item
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
						onclick={() => void coordinator?.flush()}>Retry</Button
					></Alert.Action
				></Alert.Root
			>
		</div>{/if}
	<div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
		<LiveMarkdownSurface
			value={displayBody}
			sourceRelativePath={current.relativePath}
			label="Project overview"
			onedit={(value) => coordinator?.noteEdit({ ...draft(), body: value })}
			onready={connectEditor}
		/>
	</div>
</div>
