<script lang="ts">
	import { browser } from '$app/environment';
	import {
		createLiveMarkdownDocument,
		createLiveMarkdownEditor,
		type LiveMarkdownEditor,
	} from '@noura/editor';

	let { markdown }: { markdown: string } = $props();
	let editor: LiveMarkdownEditor | null = null;

	function render(node: HTMLDivElement) {
		if (!browser || !node) {
			return;
		}
		const document = createLiveMarkdownDocument('preview', markdown);
		const handle = createLiveMarkdownEditor(node, {
			ytext: document.ytext,
			collaborative: false,
		});
		editor = handle;
		return () => {
			editor = null;
			handle.destroy();
			document.destroy();
		};
	}

	$effect(() => {
		if (browser) {
			editor?.setText(markdown);
		}
	});
</script>

<div
	{@attach render}
	class="live-md live-md--preview text-sm"
	aria-readonly="true"
></div>
