<script lang="ts">
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { siteConfig } from '$lib/site-config';
	import {
		acceptAll,
		consent,
		loadConsent,
		rejectOptional,
		saveConsent,
	} from '$lib/cookie-consent.svelte';

	const optional = siteConfig.optionalCookies;
	const hasOptional = optional.length > 0;

	onMount(loadConsent);

	const visible = $derived(
		consent.loaded && (consent.value === null || consent.settingsOpen),
	);

	// Start from the saved choice each time the settings open; edits stay local
	// until the visitor saves.
	const choices = $derived.by(() => {
		const draft = $state(
			Object.fromEntries(
				optional.map((c) => [c.id, consent.value?.granted[c.id] === true]),
			),
		);
		return draft;
	});
</script>

{#if visible}
	<section class="banner" aria-labelledby="cookie-title" aria-live="polite">
		<h2 id="cookie-title">Cookies on noura.app</h2>
		{#if hasOptional}
			<p>
				We use essential storage to run this site. With your permission, we'd
				also use the cookies listed below. You can change your mind at any time
				from the footer.
			</p>
			{#if consent.settingsOpen}
				<ul>
					{#each optional as category (category.id)}
						<li>
							<label>
								<input type="checkbox" bind:checked={choices[category.id]} />
								<span>
									<strong>{category.label}</strong>
									{category.purpose}
								</span>
							</label>
						</li>
					{/each}
				</ul>
			{/if}
		{:else}
			<p>
				This site doesn't use tracking or advertising cookies. It only saves
				this choice in your browser.
			</p>
		{/if}
		<div class="actions">
			<a href={resolve('/cookies/')}>Cookie policy</a>
			{#if hasOptional}
				<button type="button" class="secondary" onclick={rejectOptional}>
					Reject optional
				</button>
				{#if consent.settingsOpen}
					<button type="button" onclick={() => saveConsent(choices)}>
						Save choices
					</button>
				{:else}
					<button type="button" onclick={acceptAll}>Accept all</button>
				{/if}
			{:else}
				<button type="button" onclick={() => saveConsent({})}>OK</button>
			{/if}
		</div>
	</section>
{/if}

<style>
	.banner {
		position: fixed;
		left: 24px;
		bottom: 24px;
		z-index: 60;
		width: min(420px, calc(100vw - 32px));
		padding: 22px 22px 18px;
		border-radius: 20px;
		background: #141416;
		color: #ecebef;
		box-shadow:
			0 24px 60px rgba(0, 0, 0, 0.3),
			inset 0 0 0 1px rgba(255, 255, 255, 0.08);
	}
	h2 {
		margin: 0 0 8px;
		font-size: 16px;
		font-weight: 700;
		letter-spacing: -0.01em;
	}
	p {
		margin: 0;
		color: #b3b1b8;
		font-size: 14px;
		line-height: 1.55;
	}
	ul {
		margin: 14px 0 0;
		padding: 0;
		list-style: none;
	}
	li + li {
		margin-top: 10px;
	}
	label {
		display: flex;
		gap: 10px;
		color: #c9c8ce;
		font-size: 14px;
		line-height: 1.45;
	}
	label strong {
		display: block;
		color: #ecebef;
	}
	input {
		margin-top: 3px;
		accent-color: var(--lavender);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: flex-end;
		gap: 10px;
		margin-top: 18px;
	}
	.actions a {
		margin-right: auto;
		color: #c4c2ff;
		font-size: 14px;
	}
	button {
		min-height: 36px;
		padding: 0 16px;
		border: 0;
		border-radius: 999px;
		background: var(--lavender);
		color: #111114;
		font-size: 14px;
		font-weight: 700;
		cursor: pointer;
	}
	button.secondary {
		background: rgba(255, 255, 255, 0.1);
		color: #ecebef;
	}
	@media (max-width: 600px) {
		.banner {
			left: 16px;
			bottom: 16px;
		}
	}
</style>
