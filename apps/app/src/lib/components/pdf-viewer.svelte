<script lang="ts">
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { LiveProjection } from '$lib/live-refresh';
	import { pdfHref, type PdfPosition } from '$lib/pdf/navigation';
	import type { PdfHandle, PdfStatus } from '$lib/pdf/runtime';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as Field from '$lib/components/ui/field';
	import * as Alert from '$lib/components/ui/alert';

	let {
		relativePath,
		initialPosition = { page: 1, scale: 'page-width' },
		embedded = false,
		onposition,
	}: {
		relativePath: string;
		initialPosition?: PdfPosition;
		embedded?: boolean;
		onposition?: (position: PdfPosition) => void;
	} = $props();
	let status = $state<PdfStatus>({
		page: 1,
		pages: 0,
		scale: 'page-width',
		matches: '',
	});
	let handle = $state.raw<PdfHandle | null>(null);
	let loading = $state(true);
	let error = $state('');
	let passwordSubmit = $state.raw<((value: string) => void) | null>(null);
	let password = $state('');
	let passwordWrong = $state(false);
	let query = $state('');
	let thumbnails = $state(false);
	let retry: () => void = () => {};

	function attachViewer(node: HTMLDivElement) {
		const path = relativePath;
		const workspaceId = workspace.state?.workspaceId;
		return untrack(() => {
			const client = getNouraClient();
			let disposed = false;
			let version: string | undefined;
			let position = { ...initialPosition };
			let controller: AbortController | undefined;
			let refreshSequence = 0;
			const fail = (cause: unknown) => {
				controller?.abort();
				handle = null;
				version = undefined;
				loading = false;
				passwordSubmit = null;
				error =
					cause &&
					typeof cause === 'object' &&
					'message' in cause &&
					typeof cause.message === 'string'
						? cause.message
						: 'The PDF could not be opened. It may have moved or been deleted.';
			};
			const refresh = async () => {
				const sequence = ++refreshSequence;
				try {
					const data = await client.files.inspectPdf({ relativePath: path });
					if (
						disposed ||
						sequence !== refreshSequence ||
						workspace.state?.workspaceId !== workspaceId
					)
						return;
					if (version === data.version) return;
					controller?.abort();
					controller = new AbortController();
					const signal = controller.signal;
					handle = null;
					passwordSubmit = null;
					password = '';
					loading = true;
					error = '';
					const { mountPdf } = await import('$lib/pdf/runtime');
					if (disposed || signal.aborted) return;
					const mounted = await mountPdf(node, data, {
						readRange: (input) => client.files.readPdfRange(input),
						onfailure: (cause) => {
							if (!disposed && !signal.aborted) fail(cause);
						},
						position,
						signal,
						onstatus: (next) => {
							status = next;
							position = { page: next.page, scale: next.scale };
							onposition?.(position);
						},
						onpassword: (submit, wrong) => {
							passwordSubmit = submit;
							passwordWrong = wrong;
							loading = false;
						},
						openExternal: (url) => client.files.openPdfLink(url),
					});
					if (disposed || signal.aborted) {
						mounted.destroy();
						return;
					}
					handle = mounted;
					version = data.version;
					loading = false;
					passwordSubmit = null;
				} catch (cause) {
					if (disposed || sequence !== refreshSequence) return;
					fail(cause);
				}
			};
			const projection = new LiveProjection({
				refresh,
				subscribe: (handler) => client.events.subscribe(handler),
				workspaceId: () => workspaceId,
				focusSource: window,
				visibilitySource: document,
			});
			retry = () => {
				version = undefined;
				void refresh();
			};
			void projection.start().catch(() => {
				if (!disposed) {
					loading = false;
					error = 'Could not watch this PDF. Retry to open it.';
				}
			});
			return () => {
				disposed = true;
				refreshSequence++;
				projection.dispose();
				controller?.abort();
				handle = null;
				passwordSubmit = null;
				password = '';
			};
		});
	}

	function attachThumbnail(
		canvas: HTMLCanvasElement,
		page: number,
		viewer: PdfHandle,
	) {
		let release: (() => void) | undefined;
		const observer = new IntersectionObserver(([entry]) => {
			if (entry?.isIntersecting) {
				release ??= viewer.thumbnail(canvas, page);
			} else {
				release?.();
				release = undefined;
			}
		});
		observer.observe(canvas);
		return () => {
			observer.disconnect();
			release?.();
		};
	}
</script>

<div
	class="pdf-surface flex h-full min-h-0 flex-col bg-muted/40"
	aria-label="PDF viewer"
>
	<div
		class="flex shrink-0 flex-wrap items-center gap-1 border-b bg-background p-2"
		role="toolbar"
		aria-label="PDF navigation"
	>
		{#if !embedded}
			<Button
				size="sm"
				variant="ghost"
				disabled={!handle}
				aria-pressed={thumbnails}
				onclick={() => (thumbnails = !thumbnails)}>Thumbnails</Button
			>
		{/if}
		<Button
			size="sm"
			variant="ghost"
			aria-label="Previous page"
			disabled={!handle || status.page <= 1}
			onclick={() => handle?.go(status.page - 1)}>←</Button
		>
		<Input
			class="w-16"
			type="number"
			min="1"
			max={status.pages || 1}
			aria-label="Page number"
			value={status.page}
			disabled={!handle}
			onchange={(event) => handle?.go(Number(event.currentTarget.value))}
		/>
		<span class="text-xs text-muted-foreground">of {status.pages || '…'}</span>
		<Button
			size="sm"
			variant="ghost"
			aria-label="Next page"
			disabled={!handle || status.page >= status.pages}
			onclick={() => handle?.go(status.page + 1)}>→</Button
		>
		<Button
			size="sm"
			variant="ghost"
			aria-label="Zoom out"
			disabled={!handle}
			onclick={() => handle?.zoom(-1)}>−</Button
		>
		<Button
			size="sm"
			variant="ghost"
			aria-label="Zoom in"
			disabled={!handle}
			onclick={() => handle?.zoom(1)}>+</Button
		>
		<Button
			size="sm"
			variant="ghost"
			disabled={!handle}
			onclick={() => handle?.fit()}>Fit width</Button
		>
		{#if embedded}
			<Button
				size="sm"
				variant="ghost"
				onclick={() =>
					goto(
						pdfHref(relativePath, handle ? status.page : initialPosition.page),
					)}>Open in tab</Button
			>
		{:else}
			<Input
				class="w-36"
				type="search"
				aria-label="Search PDF"
				placeholder="Find in PDF"
				bind:value={query}
				disabled={!handle}
				oninput={() => handle?.find(query)}
				onkeydown={(event) => {
					if (event.key === 'Enter') {
						event.preventDefault();
						handle?.find(query, event.shiftKey, true);
					}
				}}
			/>
			<Button
				size="sm"
				variant="ghost"
				aria-label="Previous match"
				disabled={!handle || !query}
				onclick={() => handle?.find(query, true, true)}>↑</Button
			>
			<Button
				size="sm"
				variant="ghost"
				aria-label="Next match"
				disabled={!handle || !query}
				onclick={() => handle?.find(query, false, true)}>↓</Button
			>
			<span class="text-xs text-muted-foreground" aria-live="polite"
				>{status.matches}</span
			>
		{/if}
	</div>
	{#if error}
		<Alert.Root class="m-2 w-auto"
			><Alert.Title>PDF unavailable</Alert.Title><Alert.Description
				>{error}</Alert.Description
			><Button size="sm" variant="outline" onclick={() => retry()}>Retry</Button
			></Alert.Root
		>
	{/if}
	{#if passwordSubmit}
		<form
			class="flex items-center gap-2 p-3"
			onsubmit={(event) => {
				event.preventDefault();
				const submit = passwordSubmit;
				const value = password;
				password = '';
				passwordSubmit = null;
				loading = true;
				submit?.(value);
			}}
		>
			<Field.FieldGroup
				><Field.Field
					><Input
						type="password"
						aria-label="PDF password"
						placeholder={passwordWrong
							? 'Incorrect password. Try again'
							: 'PDF password'}
						bind:value={password}
						autocomplete="off"
					/></Field.Field
				></Field.FieldGroup
			>
			<Button type="submit" size="sm">Unlock</Button>
		</form>
	{/if}
	{#if loading}<div class="flex items-center gap-2 p-3" role="status">
			<Spinner />Loading PDF…
		</div>{/if}
	<div class="relative flex min-h-0 flex-1">
		{#if thumbnails && handle}
			<div
				class="w-32 shrink-0 overflow-y-auto border-r p-2"
				aria-label="Page thumbnails"
			>
				{#each Array.from({ length: status.pages }, (_, i) => i + 1) as number (number)}
					{@const viewer = handle}
					<button
						class="mb-2 flex w-full flex-col items-center gap-1 rounded-sm border p-1 text-xs"
						aria-label={`Go to page ${number}`}
						aria-current={status.page === number ? 'page' : undefined}
						onclick={() => viewer.go(number)}
					>
						<canvas
							style="width:100px;min-height:100px"
							{@attach (canvas) => attachThumbnail(canvas, number, viewer)}
						></canvas>{number}
					</button>
				{/each}
			</div>
		{/if}
		<div class="relative min-w-0 flex-1">
			<div
				class="pdf-container absolute inset-0 overflow-auto"
				{@attach attachViewer}
			></div>
		</div>
	</div>
</div>

<style>
	.pdf-surface :global(.pdfViewer) {
		--page-bg-color: white;
		--pdfViewer-padding-bottom: 0;
	}
	.pdf-surface :global(.pdfViewer .page) {
		margin: 8px auto;
	}
	.pdf-surface :global(.annotationLayer .fileAttachmentAnnotation),
	.pdf-surface :global(.annotationLayer input),
	.pdf-surface :global(.annotationLayer textarea),
	.pdf-surface :global(.annotationLayer select) {
		display: none;
	}
</style>
