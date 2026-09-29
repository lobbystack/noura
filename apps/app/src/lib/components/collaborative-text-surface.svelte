<script lang="ts">
	import { untrack } from 'svelte';
	import type {
		CollaborationSession,
		CollaborationStatus,
		CollaborationPresence,
	} from '@noura/editor';
	import type * as EditorRuntime from '@noura/editor/runtime';
	import * as Avatar from '$lib/components/ui/avatar/index.js';
	import { markdownAssets } from '$lib/pdf/markdown';
	import { preferences } from '$lib/preferences.svelte';
	let {
		session,
		sourceRelativePath,
		language = 'text',
		onerror,
	}: {
		session: CollaborationSession;
		sourceRelativePath?: string;
		language?: 'markdown' | 'text';
		onerror?: (error: unknown) => void;
	} = $props();
	let status = $state<CollaborationStatus>('Saved locally');
	let presence = $state.raw<CollaborationPresence[]>([]);
	type CollaborativeView = ReturnType<
		typeof EditorRuntime.createCollaborativeView
	>;
	let view = $state.raw<CollaborativeView | null>(null);
	let setSpellcheck: typeof EditorRuntime.setSpellcheck | null = null;

	// Follow the Settings toggle in the shared editor too.
	$effect(() => {
		const enabled = preferences.spellcheck;
		if (view) setSpellcheck?.(view, enabled);
	});

	function attachEditor(element: HTMLElement) {
		// Mount once per session; later prop changes must not rebuild it.
		const active = untrack(() => session);
		const options = untrack(() => ({
			language,
			sourceRelativePath,
			spellcheck: preferences.spellcheck,
		}));
		let disposed = false;
		let cleanup: (() => void) | undefined;
		const unsubscribe = active.subscribe(() => {
			status = active.status;
			presence = active.presence;
		});
		// The editor runtime loads with the first document, not the app.
		void import('@noura/editor/runtime')
			.then((runtime) => {
				if (disposed) return;
				setSpellcheck = runtime.setSpellcheck;
				const created = runtime.createCollaborativeView(
					element,
					active,
					options.language,
					[
						runtime.spellcheckExtension(options.spellcheck),
						...(options.language === 'markdown'
							? runtime.createPdfPreviewExtensions(
									markdownAssets(options.sourceRelativePath),
								)
							: []),
					],
				);
				view = created;
				cleanup = () => created.destroy();
			})
			.catch((error: unknown) => onerror?.(error));
		return () => {
			disposed = true;
			view = null;
			cleanup?.();
			unsubscribe();
			void active.flush().catch((error) => onerror?.(error));
		};
	}
	function protectDraft(event: BeforeUnloadEvent) {
		if (session.hasPending) {
			event.preventDefault();
			event.returnValue = '';
		}
	}
</script>

<svelte:window onbeforeunload={protectDraft} />
<div class="flex h-full min-h-0 flex-col">
	<div class="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
		<span role="status">{status}</span>
		{#each presence as member (member.deviceId)}
			<Avatar.Root title={member.name}>
				<Avatar.Fallback
					>{member.name.trim().slice(0, 2).toUpperCase() ||
						'?'}</Avatar.Fallback
				>
			</Avatar.Root>
		{/each}
	</div>
	<div
		class={[
			'min-h-0 flex-1 overflow-auto',
			language === 'markdown' && 'live-md',
		]}
		{@attach attachEditor}
	></div>
</div>
