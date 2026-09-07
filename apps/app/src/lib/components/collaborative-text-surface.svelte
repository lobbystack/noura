<script lang="ts">
	import type {
		CollaborationSession,
		CollaborationStatus,
		CollaborationPresence,
	} from '@noura/editor';
	import * as Avatar from '$lib/components/ui/avatar/index.js';
	import { createCollaborativeView } from '@noura/editor/runtime';
	let {
		session,
		language = 'text',
		onerror,
	}: {
		session: CollaborationSession;
		language?: 'markdown' | 'text';
		onerror?: (error: unknown) => void;
	} = $props();
	let status = $state<CollaborationStatus>('Saved locally');
	let presence = $state.raw<CollaborationPresence[]>([]);
	function attachEditor(element: HTMLElement) {
		const active = session;
		const view = createCollaborativeView(element, active, language);
		const unsubscribe = active.subscribe(() => {
			status = active.status;
			presence = active.presence;
		});
		return () => {
			view.destroy();
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
	<div class="min-h-0 flex-1 overflow-auto" {@attach attachEditor}></div>
</div>
