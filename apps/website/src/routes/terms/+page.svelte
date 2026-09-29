<script lang="ts">
	import LegalPage from '$lib/components/LegalPage.svelte';
	import { siteConfig } from '$lib/site-config';

	const legal = siteConfig.legal;
</script>

<LegalPage
	title="Terms and conditions"
	description="The rules for using the noura website and desktop app."
	path="terms/"
>
	<p class="note">
		noura is free, open source, and in early alpha. Keep backups of your files.
		You own your work, and the app keeps it in ordinary files on your device.
	</p>

	<h2>1. Agreement</h2>
	<p>
		These terms form an agreement between you and {legal.entity} ("noura", "we", "us").
		They cover the noura.app website and the noura desktop app you download from us.
		By using either, you accept these terms. If you don't accept them, don't use the
		website or the app.
	</p>

	<h2>2. The software license</h2>
	<p>
		The noura source code is available under the
		<a href={`${siteConfig.githubUrl}/blob/main/LICENSE`}>MIT license</a>. That
		license governs how you may use, copy, change, and share the code. These
		terms don't limit your rights under it. They add rules for the website and
		for the builds we publish.
	</p>

	<h2>3. Alpha software</h2>
	<p>
		We're still building noura. Features may change, break, or disappear, and
		bugs may affect your files. Keep your own backups. The app stores your
		workspace as ordinary files, so any backup tool you already use will work.
	</p>

	<h2>4. Updates</h2>
	<p>
		The desktop app checks for new versions and downloads them in the
		background. It installs an update only after you choose to restart. We sign
		every update, and the app refuses updates without a valid signature. The
		Linux <code>.deb</code> package doesn't update itself. Install new versions from
		the website.
	</p>

	<h2>5. Your content</h2>
	<p>
		You own everything you create in noura. We claim no rights to it, and we
		don't receive it. You're responsible for the content you keep in your
		workspace and for what you choose to send to AI providers or other tools.
	</p>

	<h2>6. Acceptable use</h2>
	<p>When you use the website, don't:</p>
	<ul>
		<li>break the law or infringe anyone's rights</li>
		<li>
			attack, overload, or probe our systems without permission. To report a
			security issue, follow our
			<a href={`${siteConfig.githubUrl}/blob/main/SECURITY.md`}
				>security policy</a
			>.
		</li>
		<li>get around rate limits or access controls</li>
	</ul>

	<h2>7. Third-party services</h2>
	<p>
		noura can connect to services we don't control, such as the AI providers you
		choose, AI assistants you connect through MCP, and GitHub, which hosts our
		downloads. Their own terms and privacy policies apply, and we aren't
		responsible for them.
	</p>

	<h2>8. Price</h2>
	<p>
		The app is free. If we ever offer paid services, we'll publish the price and
		terms first, and you'll pay only if you choose one.
	</p>

	<h2>9. Stopping</h2>
	<p>
		You can stop using noura at any time. Your files stay on your device and
		remain yours.
	</p>

	<h2>10. No warranty</h2>
	<p>
		To the extent the law allows, we provide noura "as is" and "as available",
		without warranties of any kind, including fitness for a particular purpose
		and non-infringement. We don't promise that noura will be error-free or
		always available.
	</p>

	<h2>11. Limit of liability</h2>
	<p>
		To the extent the law allows, noura is not liable for indirect, incidental,
		special, or consequential damages, or for lost data or profits. Our total
		liability for any claim related to the website or the app is limited to the
		greater of the amount you paid us in the 12 months before the claim or CAD
		$50.
	</p>
	<p>
		Nothing in these terms limits rights you have under consumer protection laws
		that can't be waived by contract, including Québec's Consumer Protection
		Act.
	</p>

	<h2>12. Governing law</h2>
	<p>
		The laws of {legal.governingLaw} govern these terms. If you're a consumer, you
		keep the protection of the mandatory laws of the place where you live, and you
		may bring a claim in your local courts.
	</p>

	<h2>13. Changes to these terms</h2>
	<p>
		We'll post changes on this page and update the effective date. If you keep
		using noura after a change takes effect, the new terms apply.
	</p>

	<h2>14. Contact</h2>
	<p>
		{legal.entity}, {legal.address}. Email
		<a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>.
	</p>
</LegalPage>

<style>
	code {
		font-size: 14px;
	}
</style>
