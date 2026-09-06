<script lang="ts">
	import '../app.css';
	let { children } = $props();
</script>

<svelte:head
	><title>Noura account</title><meta
		name="robots"
		content="noindex,nofollow"
	/></svelte:head
>
<main
	class="mx-auto flex min-h-screen w-full max-w-lg flex-col justify-center gap-8 px-6 py-12"
>
	<a href="/account" class="text-xl font-semibold tracking-tight">Noura</a>
	<section
		class="flex flex-col gap-6 rounded-2xl border bg-card p-6 text-card-foreground"
	>
		{@render children()}
	</section>
	<p class="text-sm text-muted-foreground">
		Your workspace stays on your devices. Signing in does not give this server
		your encryption keys.
	</p>
</main>
