<script lang="ts">
	import type { Snippet } from 'svelte';
	import { siteConfig } from '$lib/site-config';

	let {
		title,
		description,
		path,
		children,
	}: { title: string; description: string; path: string; children: Snippet } =
		$props();
</script>

<svelte:head>
	<title>{title} · noura</title>
	<meta name="description" content={description} />
	<link rel="canonical" href={`${siteConfig.marketingUrl}${path}`} />
</svelte:head>

<main id="main" class="legal">
	<header>
		<p class="kicker">Legal</p>
		<h1>{title}</h1>
		<p class="updated">Effective {siteConfig.legal.effectiveDate}</p>
	</header>
	<article>
		{@render children()}
	</article>
</main>

<style>
	.legal {
		max-width: 760px;
		margin: 0 auto;
		padding: 96px 32px 120px;
	}
	.kicker {
		margin: 0 0 14px;
		color: #6663d8;
		font-size: 14px;
		font-weight: 720;
	}
	h1 {
		margin: 0;
		font-size: clamp(38px, 5vw, 56px);
		line-height: 1.05;
		letter-spacing: -0.045em;
		font-weight: 580;
	}
	.updated {
		margin: 16px 0 0;
		color: #86848c;
		font-size: 14px;
	}
	article {
		margin-top: 56px;
		color: #3c3b41;
		font-size: 16px;
		line-height: 1.7;
	}
	article :global(h2) {
		margin: 48px 0 12px;
		color: var(--ink);
		font-size: 22px;
		line-height: 1.3;
		letter-spacing: -0.02em;
		font-weight: 680;
	}
	article :global(h3) {
		margin: 28px 0 8px;
		color: var(--ink);
		font-size: 17px;
		font-weight: 680;
	}
	article :global(p),
	article :global(ul) {
		margin: 0 0 16px;
	}
	article :global(ul) {
		padding-left: 22px;
	}
	article :global(li + li) {
		margin-top: 6px;
	}
	article :global(a) {
		color: #4f4cc4;
	}
	article :global(table) {
		width: 100%;
		margin: 8px 0 20px;
		border-collapse: collapse;
		font-size: 15px;
	}
	article :global(th),
	article :global(td) {
		padding: 10px 12px 10px 0;
		border-bottom: 1px solid var(--line);
		text-align: left;
		vertical-align: top;
	}
	article :global(th) {
		color: var(--ink);
		font-weight: 680;
	}
	article :global(.note) {
		padding: 14px 18px;
		border-radius: 12px;
		background: var(--mist);
		color: #4b4a52;
		font-size: 15px;
	}
	@media (max-width: 600px) {
		.legal {
			padding: 64px 20px 88px;
		}
		article {
			margin-top: 40px;
		}
		article :global(table) {
			display: block;
			overflow-x: auto;
		}
	}
</style>
