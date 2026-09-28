<script lang="ts">
	import { asset, resolve } from '$app/paths';
	import { siteConfig } from '$lib/site-config';
	import { openCookieSettings } from '$lib/cookie-consent.svelte';
</script>

<footer>
	<div class="footer-brand">
		<span
			><img src={asset('/logo.png')} alt="" width="26" height="26" />noura</span
		>
		<p>The open workspace for humans and AI.</p>
	</div>
	<div class="footer-links">
		<nav aria-label="Product">
			<p>Product</p>
			<a href={resolve('/#download')}>Download</a>
			<a href={siteConfig.githubUrl}>GitHub</a>
			<a href={siteConfig.releasesUrl}>Releases</a>
			<a href={`${siteConfig.githubUrl}/blob/main/SECURITY.md`}>Security</a>
		</nav>
		<nav aria-label="Legal">
			<p>Legal</p>
			<a href={resolve('/privacy/')}>Privacy policy</a>
			<a href={resolve('/terms/')}>Terms and conditions</a>
			<a href={resolve('/cookies/')}>Cookie policy</a>
			<button type="button" onclick={openCookieSettings}>Cookie settings</button
			>
			<a href={`${siteConfig.githubUrl}/blob/main/LICENSE`}>MIT license</a>
		</nav>
	</div>
	<p class="copyright">
		© {new Date().getFullYear()} noura. Free and open source.
	</p>
</footer>

<style>
	footer {
		margin: 0 24px 24px;
		padding: 48px;
		border-radius: 28px;
		background: #f6f5f8;
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 40px 70px;
	}
	.footer-brand span {
		display: inline-flex;
		align-items: center;
		gap: 10px;
		font-size: 20px;
		font-weight: 780;
		letter-spacing: -0.04em;
	}
	.footer-brand p {
		margin: 8px 0 0;
		color: #6f6d76;
		font-size: 14px;
	}
	.footer-links {
		display: flex;
		gap: 72px;
	}
	nav {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 12px;
	}
	nav p {
		margin: 0 0 4px;
		color: #86848c;
		font-size: 13px;
		font-weight: 650;
	}
	nav a,
	nav button {
		padding: 0;
		border: 0;
		background: none;
		color: #3c3b41;
		text-decoration: none;
		font-size: 14px;
		font-weight: 620;
		cursor: pointer;
	}
	nav a:hover,
	nav button:hover {
		color: var(--ink);
	}
	.copyright {
		grid-column: 1/-1;
		margin: 0;
		padding-top: 24px;
		border-top: 1px solid #e0dfe4;
		color: #86848c;
		font-size: 13px;
	}
	@media (max-width: 900px) {
		footer {
			margin: 0 10px 10px;
			grid-template-columns: 1fr;
		}
	}
	@media (max-width: 600px) {
		footer {
			padding: 32px 24px;
		}
		.footer-links {
			gap: 48px;
		}
	}
</style>
