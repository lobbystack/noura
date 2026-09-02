<script lang="ts">
	import LiveMarkdownSurface from '$lib/components/live-markdown-surface.svelte';

	let {
		markdown,
		sourceRelativePath,
	}: { markdown: string; sourceRelativePath?: string } = $props();
</script>

<LiveMarkdownSurface
	value={markdown}
	{sourceRelativePath}
	readOnly
	showToolbar={false}
	label="Markdown preview"
/>
