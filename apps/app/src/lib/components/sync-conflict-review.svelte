<script lang="ts">
	import { onMount } from 'svelte';
	import type { SyncConflict, WorkspaceSyncStatus } from '@noura/workspace';
	import { getNouraClient } from '$lib/state.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	let {
		disabled = false,
		onresolved,
	}: { disabled?: boolean; onresolved: (status: WorkspaceSyncStatus) => void } =
		$props();
	let conflicts = $state<SyncConflict[]>([]);
	let loaded = $state(false);
	let busy = $state(false);
	let error = $state('');
	let disposed = false;
	onMount(() => () => {
		disposed = true;
	});
	function message(cause: unknown): string {
		if (cause && typeof cause === 'object') {
			if ('code' in cause && cause.code === 'sync_file_changed')
				return 'The local file changed after this review. Review the conflict again before choosing a version.';
			if ('code' in cause && cause.code === 'sync_capture_required')
				return 'A local edit is waiting to be captured. Wait for synchronization to capture it, then review again.';
			if ('message' in cause && typeof cause.message === 'string')
				return cause.message;
		}
		return 'The conflict request could not be completed. Please try again.';
	}
	async function review() {
		busy = true;
		error = '';
		try {
			const values = await getNouraClient().sync.workspaceConflicts();
			if (!disposed) {
				conflicts = values;
				loaded = true;
			}
		} catch (cause) {
			if (!disposed) {
				error = message(cause);
				conflicts = [];
				loaded = false;
			}
		} finally {
			if (!disposed) busy = false;
		}
	}
	async function resolve(conflict: SyncConflict, choice: 'local' | 'remote') {
		if (disabled || busy || !conflict.canResolve) return;
		busy = true;
		error = '';
		try {
			const sync = getNouraClient().sync;
			await sync.resolveWorkspaceConflict({
				operationId: conflict.operationId,
				currentRevision: conflict.currentRevision,
				choice,
			});
			if (disposed) return;
			const values = await sync.workspaceConflicts();
			const status = await sync.workspaceStatus();
			if (!disposed) {
				conflicts = values;
				loaded = true;
				onresolved(status);
			}
		} catch (cause) {
			if (!disposed) {
				error = message(cause);
				conflicts = [];
				loaded = false;
			}
		} finally {
			if (!disposed) busy = false;
		}
	}
</script>

<div class="flex flex-col gap-3" aria-label="Synchronization conflicts">
	<div>
		<Button variant="outline" disabled={disabled || busy} onclick={review}
			>{busy ? 'Updating conflicts…' : 'Review conflicts'}</Button
		>
	</div>
	{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
	{#if loaded && conflicts.length === 0}<p
			role="status"
			class="text-xs text-muted-foreground"
		>
			No unresolved conflicts were returned.
		</p>{/if}
	{#each conflicts as conflict (conflict.operationId)}
		<section
			class="flex flex-col gap-3 rounded-xl border p-4"
			aria-label={`Conflict: ${conflict.path}`}
		>
			<h4 class="break-all text-sm font-medium">{conflict.path}</h4>
			<div class="grid gap-4 sm:grid-cols-2">
				<div class="flex min-w-0 flex-col gap-2">
					<h5 class="text-xs font-medium">Local version</h5>
					{#if conflict.localDeleted}<p class="text-xs text-muted-foreground">
							This file is deleted locally.
						</p>
					{:else if conflict.localPreview === null}<p
							class="text-xs text-muted-foreground"
						>
							Binary or text preview unavailable.
						</p>
					{:else}<pre
							class="max-h-48 overflow-auto rounded-lg bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all">{conflict.localPreview}</pre>{/if}
				</div>
				<div class="flex min-w-0 flex-col gap-2">
					<h5 class="text-xs font-medium">Remote version</h5>
					{#if conflict.remoteDeleted}<p class="text-xs text-muted-foreground">
							The remote version deletes this file.
						</p>
					{:else if conflict.remotePreview === null}<p
							class="text-xs text-muted-foreground"
						>
							Binary or text preview unavailable.
						</p>
					{:else}<pre
							class="max-h-48 overflow-auto rounded-lg bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all">{conflict.remotePreview}</pre>{/if}
				</div>
			</div>
			{#if conflict.canResolve}
				<p class="text-xs text-muted-foreground">
					Keep local publishes the local version, including a local deletion.
					Use remote replaces the local file with the remote version, or deletes
					it if the remote version is a deletion.
				</p>
				<div class="flex flex-wrap gap-2">
					<Button
						variant="outline"
						disabled={disabled || busy}
						onclick={() => resolve(conflict, 'local')}>Keep local</Button
					><Button
						disabled={disabled || busy}
						onclick={() => resolve(conflict, 'remote')}>Use remote</Button
					>
				</div>
			{:else}<p class="text-xs text-muted-foreground">
					This conflict needs manual review. A move, an identity mismatch, or a
					local change awaiting capture can prevent automatic resolution.
				</p>{/if}
		</section>
	{/each}
</div>
