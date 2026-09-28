<script lang="ts">
	import './global.css';
	import CookieBanner from '$lib/components/CookieBanner.svelte';
	import SiteFooter from '$lib/components/SiteFooter.svelte';
	import SiteHeader from '$lib/components/SiteHeader.svelte';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href="/favicon.png" type="image/png" />
	<link rel="apple-touch-icon" href="/apple-touch-icon.png" />
</svelte:head>

<SiteHeader />
{@render children()}
<SiteFooter />
<CookieBanner />
