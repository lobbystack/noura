<script lang="ts">
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import type { LiveMarkdownEditor } from '@noura/editor/types';
	import type { CoreEvent, Project, Task } from '@noura/workspace';
	import { isCoreError } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { AutosaveCoordinator } from '$lib/editor/autosave';
	import { registerPendingDraft } from '$lib/editor/pending-drafts.svelte';
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import Warning from 'phosphor-svelte/lib/Warning';
	import PropertyChoice from '$lib/components/property-choice.svelte';

	let {
		task,
		projects = [],
		onupdated,
	}: {
		task: Task;
		projects?: Project[];
		onupdated?: (task: Task) => void;
	} = $props();
	type Draft = {
		title: string;
		body: string;
		properties: Record<string, unknown>;
	};
	type Conflict = { draft: Draft; file: Task; deleted?: boolean };
	const statuses = ['todo', 'in-progress', 'done', 'cancelled'] as const;
	const priorities = ['low', 'medium', 'high', 'urgent'] as const;
	const initialTask = () => task;

	let current = $state.raw(initialTask());
	let baseTitle = $state(initialTask().title);
	let baseBody = $state(initialTask().body);
	let displayBody = $state(initialTask().body);
	let baseRevision = $state(initialTask().revision);
	let baseProperties = $state<Record<string, unknown>>({
		...initialTask().properties,
	});
	let properties = $state<Record<string, unknown>>({
		...initialTask().properties,
	});
	let title = $state(initialTask().title);
	let editor = $state.raw<LiveMarkdownEditor | null>(null);
	let coordinator = $state.raw<AutosaveCoordinator<Draft> | null>(null);
	let cleanup: (() => void) | null = null;
	let error = $state<unknown | null>(null);
	let conflict = $state.raw<Conflict | null>(null);
	let reviewOpen = $state(false);
	let confirmOpen = $state(false);
	let resolution = $state<'use-external' | 'replace-external' | null>(null);
	let resolving = $state(false);
	let message = $state<string | null>(null);

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
			baseRevision,
			baseTitle,
			baseBody,
			baseProperties,
			localTitle: value.title,
			localBody: value.body,
			localProperties: value.properties,
		};
	}

	function adoptCanonical(value: Task) {
		current = value;
		baseRevision = value.revision;
		baseTitle = value.title;
		baseBody = value.body;
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
		const result = await getNouraClient().tasks.saveDraft(input(value));
		if (result.status === 'conflict') {
			conflict = { draft: value, file: result.current as Task };
			reviewOpen = true;
			coordinator?.pause();
			return 'paused' as const;
		}
		const canonical = result.current as Task;
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
		if (result.status === 'merged' && result.body !== value.body)
			message = 'External changes merged';
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

	function editProperty(key: string, value: string) {
		const next = { ...properties };
		if (value) next[key] = value;
		else delete next[key];
		properties = next;
		coordinator?.noteEdit(draft());
		void coordinator?.flush();
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
			reviewOpen = true;
			coordinator?.pause();
			return;
		}

		try {
			const latest = await getNouraClient().tasks.get(current.id);
			if (!coordinator?.pendingEdits && !coordinator?.isWriting) {
				adoptCanonical(latest);
				syncDraft(latest);
				return;
			}

			const local = draft();
			const result = await getNouraClient().tasks.reconcileManaged(
				input(local),
			);
			if (result.status === 'conflict') {
				conflict = { draft: local, file: result.current as Task };
				reviewOpen = true;
				coordinator?.pause();
				return;
			}
			const canonical = result.current as Task;
			adoptCanonical(canonical);
			if (result.status === 'merged') {
				syncDraft({
					title: result.title,
					body: result.body,
					properties: result.properties,
				});
				message = 'External changes merged';
			}
			coordinator?.noteEdit(draft());
		} catch (value) {
			if (isCoreError(value) && value.code === 'object_not_found') {
				// The file disappeared between the event and the fetch; show the
				// same review surface the deleted event would.
				conflict = { draft: draft(), file: current, deleted: true };
				reviewOpen = true;
				coordinator?.pause();
				return;
			}
			error = value;
		}
	}

	function requestResolution(value: 'use-external' | 'replace-external') {
		resolution = value;
		confirmOpen = true;
	}

	async function resolveConflict() {
		if (!conflict || !resolution) return;
		resolving = true;
		try {
			const value = await getNouraClient().tasks.resolveManagedConflict({
				...input(conflict.draft),
				currentRevision: conflict.file.revision,
				relativePath: current.relativePath,
				created: current.created,
				resolution,
			});
			adoptCanonical(value as Task);
			syncDraft(value as Task);
			coordinator?.acceptDurable();
			coordinator?.resume();
			conflict = null;
			reviewOpen = false;
			confirmOpen = false;
		} catch (value) {
			error = value;
		} finally {
			resolving = false;
		}
	}

	function errorMessage(value: unknown) {
		if (value instanceof Error) return value.message;
		if (value && typeof value === 'object' && 'message' in value)
			return String(value.message);
		return 'Noura could not save this task.';
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
	<header class="flex min-h-16 items-center px-6">
		<input
			bind:value={title}
			oninput={() => coordinator?.noteEdit(draft())}
			onblur={() => void coordinator?.flush()}
			aria-label="Task title"
			placeholder="Task title"
			class="min-w-0 flex-1 bg-transparent text-base font-semibold outline-none placeholder:text-muted-foreground"
		/>
	</header>
	<Separator />
	<div class="flex flex-wrap items-center gap-3 px-6 py-3 text-sm">
		<PropertyChoice
			label="Status"
			options={statuses}
			size="sm"
			class="w-36"
			value={String(properties.status ?? 'todo')}
			onchange={(value) => value && editProperty('status', value)}
		/>
		<PropertyChoice
			label="Priority"
			options={priorities}
			size="sm"
			class="w-32"
			value={String(properties.priority ?? 'medium')}
			onchange={(value) => value && editProperty('priority', value)}
		/>
		<label class="flex items-center gap-2 text-xs text-muted-foreground"
			><span>Due</span><input
				type="date"
				value={typeof properties.due === 'string' ? properties.due : ''}
				onchange={(event) => editProperty('due', event.currentTarget.value)}
				class="rounded-md border border-input bg-transparent px-2 py-1"
			/></label
		>
		<Select.Root
			type="single"
			value={typeof properties.project === 'string' ? properties.project : ''}
			onValueChange={(value) => editProperty('project', value)}
			><Select.Trigger size="sm" class="w-44" aria-label="Project"
				>{projects.find((project) => project.id === properties.project)
					?.title ?? 'No project'}</Select.Trigger
			><Select.Content
				><Select.Group
					><Select.Item value="" label="No project">No project</Select.Item
					>{#each projects as project (project.id)}<Select.Item
							value={project.id}
							label={project.title}>{project.title}</Select.Item
						>{/each}</Select.Group
				></Select.Content
			></Select.Root
		>
		{#if message}<span class="text-xs text-muted-foreground" role="status"
				>{message}</span
			>{/if}
	</div>
	{#if conflict}<div class="px-6 pb-3">
			<Alert.Root
				><Warning /><Alert.Title
					>{conflict.deleted
						? 'This task file was deleted'
						: 'This task changed in another app'}</Alert.Title
				><Alert.Description
					>{conflict.deleted
						? 'Your draft is still available and can restore the file.'
						: 'Review both versions before choosing which one to keep.'}</Alert.Description
				><Alert.Action
					><Button
						variant="outline"
						size="sm"
						onclick={() => (reviewOpen = true)}>Review conflict</Button
					></Alert.Action
				></Alert.Root
			>
		</div>{/if}
	{#if error}<div class="px-6 pb-3">
			<Alert.Root variant="destructive"
				><Warning /><Alert.Title>Changes could not be saved</Alert.Title
				><Alert.Description>{errorMessage(error)}</Alert.Description
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
			label="Task body"
			onedit={(body) => coordinator?.noteEdit({ ...draft(), body })}
			onready={connectEditor}
		/>
	</div>
</div>

{#if conflict}<Sheet.Root bind:open={reviewOpen}
		><Sheet.Content class="sm:max-w-2xl"
			><Sheet.Header
				><Sheet.Title
					>{conflict.deleted
						? 'Restore deleted task'
						: 'Review task conflict'}</Sheet.Title
				><Sheet.Description
					>{conflict.deleted
						? 'Restore the task file from your current draft.'
						: 'Noura preserved both versions.'}</Sheet.Description
				></Sheet.Header
			>
			<div
				class="grid min-h-0 flex-1 gap-5 overflow-y-auto px-4 pb-4 {conflict.deleted
					? 'grid-cols-1'
					: 'grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]'}"
			>
				<section>
					<h2 class="mb-3 text-sm font-medium">Your draft</h2>
					<MarkdownPreview markdown={conflict.draft.body} />
				</section>
				{#if !conflict.deleted}
					<Separator orientation="vertical" />
					<section>
						<h2 class="mb-3 text-sm font-medium">File version</h2>
						<MarkdownPreview markdown={conflict.file.body} />
					</section>
				{/if}
			</div>
			<Sheet.Footer
				><Button variant="outline" onclick={() => (reviewOpen = false)}
					>Cancel and continue reviewing</Button
				>{#if !conflict.deleted}<Button
						variant="outline"
						onclick={() => requestResolution('use-external')}
						>Use file version</Button
					>{/if}<Button onclick={() => requestResolution('replace-external')}
					>{conflict.deleted
						? 'Restore task file'
						: 'Replace file with my version'}</Button
				></Sheet.Footer
			></Sheet.Content
		></Sheet.Root
	>{/if}
<AlertDialog.Root bind:open={confirmOpen}
	><AlertDialog.Content
		><AlertDialog.Header
			><AlertDialog.Title
				>{resolution === 'use-external'
					? 'Use the file version?'
					: conflict?.deleted
						? 'Restore this task file?'
						: 'Replace the file version?'}</AlertDialog.Title
			><AlertDialog.Description
				>{conflict?.deleted
					? 'The file will be recreated at its previous workspace path.'
					: 'The displaced version is saved to recovery history first.'}</AlertDialog.Description
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
