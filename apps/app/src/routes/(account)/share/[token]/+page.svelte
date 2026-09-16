<script lang="ts">
	import { page } from '$app/state';
	import ShareViewer from '$account/share-viewer.svelte';
</script>

{#key page.url.pathname + page.url.hash}
	<ShareViewer token={page.params.token ?? ''} fragment={page.url.hash} />
{/key}
