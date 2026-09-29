<script lang="ts">
	import { renderChatMarkdown } from '$lib/ai/chat-markdown';
	import { StreamingMarkdown } from '$lib/ai/streaming-markdown';

	let {
		markdown,
		streaming = false,
		onlink,
	}: {
		markdown: string;
		/** While true, only finished blocks are rendered; the rest shows as text. */
		streaming?: boolean;
		/** Opens a link from the message. Links never navigate the app itself. */
		onlink: (href: string) => void;
	} = $props();

	const stream = new StreamingMarkdown(renderChatMarkdown);
	const view = $derived(
		streaming
			? stream.update(markdown)
			: { html: renderChatMarkdown(markdown), tail: '' },
	);

	function handleClick(event: MouseEvent) {
		const anchor =
			event.target instanceof Element ? event.target.closest('a') : null;
		if (!anchor) return;
		event.preventDefault();
		const href = anchor.getAttribute('href');
		if (href) onlink(href);
	}
</script>

<!-- Links inside are real anchors, so keyboard activation fires this click. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="chat-md" onclick={handleClick}>
	<!-- The HTML comes only from renderChatMarkdown: a strict Markdown
	allowlist sanitized by DOMPurify. -->
	{@html view.html}
	{#if view.tail}
		<p class="whitespace-pre-wrap">{view.tail}</p>
	{/if}
</div>

<style>
	.chat-md {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		font-size: var(--text-sm);
		line-height: 1.5rem;
		overflow-wrap: anywhere;
	}
	.chat-md :global(:where(h1, h2, h3, h4, h5, h6)) {
		font-size: var(--text-base);
		font-weight: 600;
	}
	.chat-md :global(:where(ul, ol)) {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		padding-left: 1.25rem;
	}
	.chat-md :global(ul) {
		list-style: disc;
	}
	.chat-md :global(ol) {
		list-style: decimal;
	}
	.chat-md :global(a) {
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 4px;
	}
	.chat-md :global(code) {
		background: var(--muted);
		border-radius: 0.25rem;
		font-family: var(--font-mono, monospace);
		font-size: var(--text-xs);
		padding: 0.05rem 0.3rem;
	}
	.chat-md :global(pre) {
		background: var(--muted);
		border-radius: 0.5rem;
		overflow-x: auto;
		padding: 0.75rem;
	}
	.chat-md :global(pre code) {
		background: none;
		padding: 0;
	}
	.chat-md :global(blockquote) {
		border-left: 2px solid var(--border);
		color: var(--muted-foreground);
		padding-left: 0.75rem;
	}
	.chat-md :global(table) {
		border-collapse: collapse;
		display: block;
		overflow-x: auto;
	}
	.chat-md :global(:where(th, td)) {
		border-bottom: 1px solid var(--border);
		padding: 0.35rem 0.6rem;
		text-align: left;
	}
</style>
