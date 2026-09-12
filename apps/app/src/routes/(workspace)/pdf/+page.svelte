<script lang="ts">
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import PdfViewer from '$lib/components/pdf-viewer.svelte';
	import { tabsStore } from '$lib/tabs.svelte';
	import { workspace } from '$lib/state.svelte';
	import { normalizePdfPage, pdfHref } from '$lib/pdf/navigation';
	const path = $derived(page.url.searchParams.get('path') ?? '');
	const requestedPage = $derived(
		page.url.searchParams.has('page')
			? normalizePdfPage(page.url.searchParams.get('page'))
			: undefined,
	);
	const tabKey = $derived(`pdf:${workspace.state?.workspaceId}:${path}`);
	const tab = $derived(
		tabsStore.tabs.find((candidate) => candidate.objectId === tabKey),
	);
	function registerTab() {
		const relativePath = path;
		const targetPage = requestedPage;
		const workspaceId = workspace.state?.workspaceId;
		untrack(() => {
			if (!relativePath || !workspaceId) return;
			tabsStore.setWorkspace(workspaceId);
			const tabId = tabsStore.open(
				`pdf:${workspaceId}:${relativePath}`,
				'pdf',
				relativePath.split('/').at(-1) ?? relativePath,
			);
			tabsStore.setLocation(tabId, pdfHref(relativePath));
			if (targetPage !== undefined)
				tabsStore.setPdfPosition(tabId, {
					page: targetPage,
					scale: tabsStore.active?.pdfPosition?.scale ?? 'page-width',
				});
		});
	}
</script>

<div class="flex min-h-0 flex-1 flex-col" {@attach registerTab}>
	{#if path && tab}
		{#key `${workspace.state?.workspaceId}:${path}:${requestedPage ?? ''}`}
			<PdfViewer
				relativePath={path}
				initialPosition={untrack(() => ({
					page: requestedPage ?? tab.pdfPosition?.page ?? 1,
					scale: tab.pdfPosition?.scale ?? 'page-width',
				}))}
				onposition={(position) => tabsStore.setPdfPosition(tab.id, position)}
			/>
		{/key}
	{:else}
		<p class="p-6 text-muted-foreground">Choose a PDF from workspace files.</p>
	{/if}
</div>
