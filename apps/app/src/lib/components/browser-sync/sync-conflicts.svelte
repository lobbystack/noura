<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import type { BrowserSyncConflictDetail } from '$lib/browser-sync';
	import { conflictReasonText, count, errorText } from './copy';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let resolving = $state<{ id: string; choice: 'local' | 'remote' } | null>(
		null,
	);
	let error = $state('');
	let notice = $state('');

	const conflicts = $derived(panel.summary?.conflictDetails ?? []);

	async function resolve(
		conflict: BrowserSyncConflictDetail,
		choice: 'local' | 'remote',
	) {
		const controller = panel.controller;
		if (!controller || resolving) return;
		resolving = { id: conflict.operationId, choice };
		error = '';
		notice = '';
		try {
			const result = await controller.resolveConflict(
				conflict.operationId,
				choice,
			);
			if (!result.ok) {
				error = result.message;
				return;
			}
			await panel.refresh();
			const kept =
				choice === 'local' ? 'this browser’s version' : 'the synced version';
			notice = `Kept ${kept} of ${conflict.path}. ${count(result.value.remaining, 'conflict', 'conflicts')} left.`;
		} catch (cause) {
			error = errorText(cause, "The conflict couldn't be resolved.");
		} finally {
			resolving = null;
		}
	}
</script>

{#if conflicts.length > 0 || notice || error}
	<div
		class="flex flex-col gap-3"
		aria-labelledby="browser-sync-conflicts-heading"
	>
		<h4 id="browser-sync-conflicts-heading" class="text-sm font-medium">
			Conflicts
		</h4>
		{#if conflicts.length > 0}
			<p class="text-xs text-muted-foreground">
				noura won't change these files until you pick a version.
			</p>
		{/if}
		{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
		{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
		<ul class="flex flex-col gap-3">
			{#each conflicts as conflict (conflict.operationId)}
				<li class="flex flex-col gap-3 rounded-xl border p-4">
					<div class="flex flex-col gap-1">
						<p class="font-mono text-xs break-all select-text">
							{conflict.path}
						</p>
						<p class="text-xs text-muted-foreground">
							{conflictReasonText(conflict.reason)}
						</p>
					</div>
					<div class="flex flex-wrap gap-2">
						<Button
							variant="outline"
							disabled={panel.busy || resolving !== null}
							onclick={() => resolve(conflict, 'local')}
						>
							{#if resolving?.id === conflict.operationId && resolving.choice === 'local'}
								<Spinner data-icon="inline-start" />
							{/if}
							Keep this browser’s version
						</Button>
						<Button
							variant="outline"
							disabled={panel.busy || resolving !== null}
							onclick={() => resolve(conflict, 'remote')}
						>
							{#if resolving?.id === conflict.operationId && resolving.choice === 'remote'}
								<Spinner data-icon="inline-start" />
							{/if}
							Use the synced version
						</Button>
					</div>
				</li>
			{/each}
		</ul>
	</div>
{/if}
