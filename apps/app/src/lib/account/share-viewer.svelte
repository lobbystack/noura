<script lang="ts">
	import { onMount } from 'svelte';
	let { token, fragment }: { token: string; fragment: string } = $props();
	import {
		loadShare,
		shareKeys,
		ShareError,
		type SharePayload,
	} from '$account/share';
	import { shareMarkdown } from '$account/share-markdown';
	let payload = $state<SharePayload | null>(null);
	let html = $state('');
	let error = $state('');
	let loaded = $state(false);
	onMount(() => {
		const render = shareMarkdown(window);
		let stopped = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		let controller: AbortController | undefined;
		let previous = '';
		let keys: ReturnType<typeof shareKeys>;
		try {
			keys = shareKeys(fragment);
		} catch (cause) {
			error =
				cause instanceof ShareError
					? cause.message
					: 'This share link is incomplete.';
			loaded = true;
			return;
		}
		async function refresh() {
			if (stopped || document.hidden || controller) return;
			controller = new AbortController();
			try {
				const value = await loadShare(token, keys, controller.signal);
				if (stopped || controller.signal.aborted) return;
				if (previous && BigInt(value.revision) < BigInt(previous))
					throw new ShareError(
						'An older version was returned. Ask the owner to refresh this share.',
					);
				if (value.revision !== previous || !payload) {
					html = render(value.payload.markdown);
					payload = value.payload;
					previous = value.revision;
				}
				error = '';
				loaded = true;
			} catch (cause) {
				if (stopped || controller.signal.aborted) return;
				payload = null;
				html = '';
				loaded = true;
				error =
					cause instanceof ShareError
						? cause.message
						: 'Unable to check this shared item. Reconnecting…';
			} finally {
				controller = undefined;
				if (!stopped && !document.hidden) timer = setTimeout(refresh, 5000);
			}
		}
		function visibility() {
			clearTimeout(timer);
			if (document.hidden) controller?.abort();
			else void refresh();
		}
		document.addEventListener('visibilitychange', visibility);
		void refresh();
		return () => {
			stopped = true;
			clearTimeout(timer);
			controller?.abort();
			document.removeEventListener('visibilitychange', visibility);
			keys.key.fill(0);
			keys.signer.fill(0);
		};
	});
</script>

<svelte:head><title>Shared item · Noura</title></svelte:head>
<p class="text-sm text-muted-foreground">Shared with you · Read only</p>
{#if error}<p role="alert">{error}</p>
{:else if payload}
	<h1 class="shared-title break-words font-semibold">
		{payload.title || 'Untitled'}
	</h1>
	<p class="text-xs text-muted-foreground">
		Updated <time datetime={payload.updatedAt}
			>{new Date(payload.updatedAt).toLocaleString()}</time
		>
	</p>
	<!-- HTML is generated only by the strict Markdown allowlist and DOMPurify in share-markdown.ts. -->
	<article
		class="shared-markdown"
		data-sveltekit-preload-data="off"
		data-sveltekit-preload-code="off"
	>
		<!-- eslint-disable-next-line svelte/no-at-html-tags -->
		{@html html}
	</article>
	<p class="text-xs text-muted-foreground">
		Checks for updates every five seconds while this page is visible. Embedded
		images are omitted.
	</p>
{:else if !loaded}<p role="status">Opening encrypted share…</p>{/if}

<style>
	.shared-title {
		font-size: var(--content-heading-1-size);
	}
	.shared-markdown {
		overflow-wrap: anywhere;
		font-size: var(--content-text-size);
		line-height: 1.7;
	}
	.shared-markdown :global(p),
	.shared-markdown :global(ul),
	.shared-markdown :global(ol),
	.shared-markdown :global(pre),
	.shared-markdown :global(blockquote),
	.shared-markdown :global(table) {
		margin-block: 1rem;
	}
	.shared-markdown :global(h1),
	.shared-markdown :global(h2),
	.shared-markdown :global(h3),
	.shared-markdown :global(h4) {
		font-weight: 600;
		margin-block: 1.5rem 0.5rem;
	}
	.shared-markdown :global(h1) {
		font-size: var(--content-heading-1-size);
	}
	.shared-markdown :global(h2) {
		font-size: var(--content-heading-2-size);
	}
	.shared-markdown :global(h3) {
		font-size: var(--content-heading-3-size);
	}
	.shared-markdown :global(h4) {
		font-size: var(--content-heading-4-size);
	}
	.shared-markdown :global(a) {
		text-decoration: underline;
	}
	.shared-markdown :global(ul) {
		list-style: disc;
		padding-left: 1.5rem;
	}
	.shared-markdown :global(ol) {
		list-style: decimal;
		padding-left: 1.5rem;
	}
	.shared-markdown :global(pre) {
		overflow: auto;
		background: var(--muted);
		padding: 1rem;
		border-radius: 0.5rem;
	}
	.shared-markdown :global(blockquote) {
		border-left: 2px solid var(--border);
		padding-left: 1rem;
	}
	.shared-markdown :global(th),
	.shared-markdown :global(td) {
		border: 1px solid var(--border);
		padding: 0.5rem;
	}
</style>
