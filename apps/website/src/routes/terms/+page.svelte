<script lang="ts">
	import LegalPage from '$lib/components/LegalPage.svelte';
	import { siteConfig } from '$lib/site-config';

	const legal = siteConfig.legal;
</script>

<LegalPage
	title="Terms and conditions"
	description="The rules for using the noura website, desktop app, and noura Sync."
	path="terms/"
>
	<p class="note">
		noura is free, open source, and in early alpha. Keep backups of your files.
		You own your work, and the app keeps it in ordinary files on your device.
	</p>

	<h2>1. Agreement</h2>
	<p>
		These terms form an agreement between you and {legal.entity} ("noura", "we", "us").
		They cover the noura.app website, the noura desktop app you download from us,
		and the optional noura Sync service. By using any of them, you accept these terms.
		If you don't accept them, don't use the services.
	</p>

	<h2>2. The software license</h2>
	<p>
		The noura source code is available under the
		<a href={`${siteConfig.githubUrl}/blob/main/LICENSE`}>MIT license</a>. That
		license governs how you may use, copy, change, and share the code. These
		terms don't limit your rights under it. They add rules for the website and
		for noura Sync, which we operate.
	</p>

	<h2>3. Alpha software</h2>
	<p>
		noura is in active development. Features may change, break, or disappear,
		and bugs may affect your files. Keep your own backups. The app stores your
		workspace as ordinary files, so any backup tool you already use will work.
	</p>

	<h2>4. Updates</h2>
	<p>
		The desktop app checks for new versions and downloads them in the
		background. It installs an update only after you choose to restart. We sign
		every update, and the app refuses updates without a valid signature.
	</p>

	<h2>5. noura Sync accounts</h2>
	<p>
		noura Sync is optional and currently available by invitation. To use it:
	</p>
	<ul>
		<li>give us an email address you control</li>
		<li>keep your devices, passkeys, and recovery kit secure</li>
		<li>tell us right away if you think someone accessed your account</li>
	</ul>
	<p>
		Your devices encrypt your workspace, and we don't hold your keys. If you
		lose every approved device and your recovery kit, we can't recover your
		synced data. Your local files stay on your devices.
	</p>

	<h2>6. Your content</h2>
	<p>
		You own everything you create in noura. We claim no rights to it. For noura
		Sync, you let us store and transmit your encrypted data only as needed to
		run the service. You're responsible for the content you store and for who
		you share it with, including through share links.
	</p>

	<h2>7. Acceptable use</h2>
	<p>When you use noura Sync or the website, don't:</p>
	<ul>
		<li>break the law or infringe anyone's rights</li>
		<li>store or share malware, or content that exploits or harms children</li>
		<li>
			attack, overload, or probe our systems without permission. To report a
			security issue, follow our
			<a href={`${siteConfig.githubUrl}/blob/main/SECURITY.md`}
				>security policy</a
			>.
		</li>
		<li>get around storage limits, rate limits, or access controls</li>
		<li>use someone else's account without permission</li>
	</ul>
	<p>
		We may suspend or close a Sync account that breaks these rules. When the
		situation allows, we'll warn you first and give you time to move your data.
	</p>

	<h2>8. Third-party services</h2>
	<p>
		noura can connect to services we don't control, such as AI providers you
		choose and GitHub, which hosts our downloads. Their own terms and privacy
		policies apply, and we aren't responsible for them.
	</p>

	<h2>9. Price</h2>
	<p>
		The app and noura Sync are free today. If we introduce paid plans, we'll
		announce the price and terms in advance. You'll only pay if you choose a
		paid plan.
	</p>

	<h2>10. Changes and ending the service</h2>
	<p>
		We may change or discontinue noura Sync. Before a change that removes access
		to your synced data, we'll give at least [notice period, for example 30
		days] of notice so you can keep a copy. You can stop using noura at any
		time. To delete your Sync account, contact us. Your local files remain yours
		either way.
	</p>

	<h2>11. No warranty</h2>
	<p>
		To the extent the law allows, we provide noura "as is" and "as available",
		without warranties of any kind, including fitness for a particular purpose
		and non-infringement. We don't promise that noura will be error-free or
		always available.
	</p>

	<h2>12. Limit of liability</h2>
	<p>
		To the extent the law allows, noura is not liable for indirect, incidental,
		special, or consequential damages, or for lost data or profits. Our total
		liability for any claim related to the services is limited to the greater of
		the amount you paid us in the 12 months before the claim or CAD $50.
	</p>
	<p>
		Nothing in these terms limits rights you have under consumer protection laws
		that can't be waived by contract, including Québec's Consumer Protection
		Act.
	</p>

	<h2>13. Governing law</h2>
	<p>
		These terms are governed by the laws of {legal.governingLaw}. If you're a
		consumer, you keep the protection of the mandatory laws of the place where
		you live, and you may bring a claim in your local courts.
	</p>

	<h2>14. Changes to these terms</h2>
	<p>
		We'll post changes on this page and update the effective date. For
		significant changes, we'll notify Sync users by email at least 30 days
		ahead. If you keep using noura after a change takes effect, the new terms
		apply.
	</p>

	<h2>15. Contact</h2>
	<p>
		{legal.entity}, {legal.address}. Email
		<a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>.
	</p>
</LegalPage>
