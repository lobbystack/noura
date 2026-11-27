<script lang="ts">
	import { resolve } from '$app/paths';
	import Icon from './Icon.svelte';
	import { siteConfig } from '$lib/site-config';
</script>

<a class="skip-link" href="#main">Skip to content</a>

<header class="site-header">
	<div class="header-inner">
		<a class="brand" href={resolve('/')} aria-label="noura home">noura</a>
		<nav aria-label="Primary navigation">
			<a
				class="nav-icon"
				href={siteConfig.githubUrl}
				aria-label="noura on GitHub"
			>
				<Icon name="github" size={22} />
			</a>
			<a class="download" href={resolve('/#download')}>Download</a>
		</nav>
	</div>
</header>

<style>
	.skip-link {
		position: absolute;
		left: 16px;
		top: -60px;
		z-index: 100;
		padding: 10px 16px;
		border-radius: 999px;
		background: var(--ink);
		color: white;
		text-decoration: none;
	}
	.skip-link:focus {
		top: 12px;
	}
	.site-header {
		position: sticky;
		top: 0;
		z-index: 50;
		background: rgba(255, 255, 255, 0.82);
		backdrop-filter: saturate(1.4) blur(14px);
		-webkit-backdrop-filter: saturate(1.4) blur(14px);
	}
	.header-inner {
		height: 72px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		max-width: 1440px;
		margin: 0 auto;
		padding: 0 32px;
	}
	.brand {
		text-decoration: none;
		font-size: 22px;
		font-weight: 780;
		line-height: 1;
		letter-spacing: -0.045em;
	}
	nav {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.nav-icon {
		display: grid;
		width: 40px;
		height: 40px;
		place-items: center;
		border-radius: 999px;
		color: #3c3b41;
		transition:
			color 180ms ease,
			background 180ms ease;
	}
	.nav-icon:hover {
		background: rgba(15, 15, 16, 0.06);
		color: var(--ink);
	}
	.download {
		display: inline-flex;
		min-height: 40px;
		align-items: center;
		padding: 0 20px;
		border-radius: 999px;
		background: var(--ink);
		color: white;
		text-decoration: none;
		font-size: 14px;
		font-weight: 700;
		transition: transform 180ms ease;
	}
	.download:hover {
		transform: translateY(-2px);
	}
	@media (max-width: 900px) {
		.header-inner {
			padding: 0 18px;
		}
	}
	@media (max-width: 600px) {
		.header-inner {
			height: 64px;
		}
		.download {
			min-height: 38px;
			padding: 0 16px;
			font-size: 13px;
		}
	}
</style>
