<script lang="ts">
	import { resolve } from '$app/paths';
	import LegalPage from '$lib/components/LegalPage.svelte';
	import { openCookieSettings } from '$lib/cookie-consent.svelte';
	import { siteConfig } from '$lib/site-config';

	const legal = siteConfig.legal;
</script>

<LegalPage
	title="Cookie policy"
	description="The cookies and browser storage noura uses, and how to control them."
	path="cookies/"
>
	<p class="note">
		noura.app uses no tracking or advertising cookies. We don't use analytics
		cookies either.
	</p>

	<h2>What cookies are</h2>
	<p>
		Cookies are small files a website saves in your browser. Similar tools, such
		as local storage, keep information on your device in the same way. This
		policy covers both.
	</p>

	<h2>On noura.app</h2>
	<p>We store one item, and only in your own browser:</p>
	<table>
		<thead>
			<tr><th>Name</th><th>Type</th><th>Purpose</th><th>Kept for</th></tr>
		</thead>
		<tbody>
			<tr>
				<td><code>noura-cookie-consent</code></td>
				<td>Local storage, essential</td>
				<td>Remembers your cookie choice so the banner doesn't return</td>
				<td>Until you clear your browser data</td>
			</tr>
		</tbody>
	</table>
	{#if siteConfig.optionalCookies.length > 0}
		<p>With your consent, we also use:</p>
		<ul>
			{#each siteConfig.optionalCookies as category (category.id)}
				<li><strong>{category.label}:</strong> {category.purpose}</li>
			{/each}
		</ul>
	{/if}

	<h2>On noura Sync pages</h2>
	<p>
		If you sign in to noura Sync in your browser, the service uses essential
		storage to make that work:
	</p>
	<table>
		<thead>
			<tr><th>What</th><th>Type</th><th>Purpose</th><th>Kept for</th></tr>
		</thead>
		<tbody>
			<tr>
				<td>Session cookie</td>
				<td>Cookie, essential</td>
				<td>Keeps you signed in</td>
				<td>7 days, or until you sign out</td>
			</tr>
			<tr>
				<td>Sign-in challenge</td>
				<td>Cookie, essential</td>
				<td>Completes a passkey sign-in securely</td>
				<td>A few minutes</td>
			</tr>
			<tr>
				<td>Browser workspace and device keys</td>
				<td>Browser file storage and local storage, essential</td>
				<td>
					Holds your browser workspace, its encryption keys, and your plugin
					order on your device
				</td>
				<td>Until you remove the workspace or clear your browser data</td>
			</tr>
		</tbody>
	</table>
	<p>
		Your browser sends the cookies only to noura Sync. The browser workspace
		stays on your device, and we can't read the encrypted data it syncs.
	</p>

	<h2>Other sites</h2>
	<p>
		Links to GitHub and other sites take you to services with their own cookie
		policies. GitHub may set cookies when you download noura or visit our
		repository.
	</p>

	<h2>Your choices</h2>
	<p>
		<button type="button" class="link" onclick={openCookieSettings}
			>Open cookie settings</button
		> to review your choice. You can also block or delete cookies and local storage
		in your browser settings. If you block essential storage, noura Sync sign-in won't
		work.
	</p>

	<h2>Questions</h2>
	<p>
		Email <a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>. The
		<a href={resolve('/privacy/')}>privacy policy</a> explains how we handle personal
		information.
	</p>
</LegalPage>

<style>
	.link {
		padding: 0;
		border: 0;
		background: none;
		color: #4f4cc4;
		font: inherit;
		text-decoration: underline;
		cursor: pointer;
	}
	code {
		font-size: 14px;
	}
</style>
