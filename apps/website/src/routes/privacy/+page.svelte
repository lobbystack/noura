<script lang="ts">
	import { resolve } from '$app/paths';
	import LegalPage from '$lib/components/LegalPage.svelte';
	import { siteConfig } from '$lib/site-config';

	const legal = siteConfig.legal;
</script>

<LegalPage
	title="Privacy policy"
	description="What noura collects, why, and the choices you have."
	path="privacy/"
>
	<p class="note">
		noura runs on your computer. Your notes, tasks, and projects stay in files
		on your device. We don't receive them, the app sends us no analytics, and we
		don't sell or rent personal information.
	</p>

	<h2>Who we are</h2>
	<p>
		{legal.entity} ("noura", "we", "us") publishes the noura website and the noura
		desktop app. We're based in {legal.address}.
	</p>
	<p>
		{legal.privacyOfficer}, is responsible for protecting personal information
		and answers requests at
		<a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>.
	</p>

	<h2>What this policy covers</h2>
	<ul>
		<li>The website at noura.app</li>
		<li>The noura desktop app for macOS, Windows, and Linux</li>
	</ul>
	<p>
		noura has no accounts and no cloud service. You don't sign up to use it.
	</p>

	<h2>What we collect</h2>

	<h3>When you visit noura.app</h3>
	<p>
		The site sets no tracking or advertising cookies and runs no analytics. Our
		hosting provider, {legal.websiteHost}, records standard server logs: your IP
		address, browser type, the page you requested, and the time. We use these
		logs to keep the site running and secure.
	</p>
	<p>
		Your browser stores your cookie choice locally. See the <a
			href={resolve('/cookies/')}>cookie policy</a
		>.
	</p>

	<h3>When you download or update the app</h3>
	<p>
		GitHub hosts our installers and update files. When you download noura, or
		when the app checks for an update, GitHub receives your IP address and basic
		request details. The app checks shortly after it starts and every 6 hours.
		The check sends no workspace or usage data. The Linux <code>.deb</code>
		package doesn't check for updates. GitHub handles this information under the
		<a
			href="https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement"
			>GitHub Privacy Statement</a
		>.
	</p>

	<h3>When you use the desktop app</h3>
	<p>
		Your workspace is a folder on your device. We don't receive its contents,
		and the app sends us no usage analytics or crash reports.
	</p>
	<ul>
		<li>
			<strong>AI chat.</strong> AI stays off until you connect an AI provider you
			choose. When you chat, noura sends your messages from your device to that provider.
			It sends workspace content only after you allow it for that provider, and it
			asks before each tool the assistant wants to run. The provider's own privacy
			policy applies. We don't receive any of it.
		</li>
		<li>
			<strong>API keys.</strong> noura stores provider keys in your operating system's
			credential store. They never reach us.
		</li>
		<li>
			<strong>External AI tools.</strong> If you connect an AI assistant to noura
			through the Model Context Protocol (MCP), that assistant can read and change
			your workspace on your device. noura doesn't ask before each of its actions.
			The assistant's own privacy policy covers what it sends to its provider.
		</li>
	</ul>

	<h3>When you email us</h3>
	<p>
		If you write to us, we receive your email address and your message. We use
		them to reply.
	</p>

	<h2>How we use information</h2>
	<p>We use personal information to:</p>
	<ul>
		<li>run, secure, and fix the website and the app</li>
		<li>answer your messages</li>
		<li>prevent abuse</li>
		<li>meet our legal obligations</li>
	</ul>
	<p>
		We don't use your information for advertising, and we don't build profiles
		about you.
	</p>
	<p>
		If the GDPR applies to you, we rely on two legal bases: our legitimate
		interest in running and securing the website and answering your messages,
		and our legal obligations.
	</p>

	<h2>Who we share it with</h2>
	<p>We share personal information only with:</p>
	<ul>
		<li>
			service providers that run parts of noura for us: {legal.websiteHost} hosts
			the website, GitHub hosts downloads and updates, and our email provider delivers
			the messages you send us
		</li>
		<li>authorities, when the law requires it</li>
		<li>
			a buyer or successor, if noura is sold or merged, under the same
			protections described here
		</li>
	</ul>
	<p>We never sell or rent personal information.</p>

	<h2>Where we store it</h2>
	<p>
		Our service providers may process information outside your province or
		country, including in the United States. When they do, we use contracts and
		safeguards that protect your information as this policy describes.
	</p>

	<h2>How long we keep it</h2>
	<ul>
		<li>Website server logs: 30 days</li>
		<li>Emails you send us: as long as we need them to answer you</li>
	</ul>

	<h2>How we protect it</h2>
	<p>
		We don't store your workspace, so our systems can't expose it. We limit
		staff access to the systems that hold personal information, and the website
		uses encrypted connections. If a breach creates a risk of serious harm,
		we'll notify you and the relevant authorities as the law requires.
	</p>

	<h2>Your rights</h2>
	<p>Depending on where you live, you can ask us to:</p>
	<ul>
		<li>
			tell you what personal information we hold about you and give you a copy
		</li>
		<li>correct information that's wrong</li>
		<li>delete your information</li>
		<li>give you your information in a portable format</li>
		<li>stop a particular use, or withdraw consent you gave</li>
	</ul>
	<p>
		Email <a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>. We
		reply within 30 days. You can also complain to your privacy regulator, such
		as the Commission d'accès à l'information du Québec, the Office of the
		Privacy Commissioner of Canada, or your local data protection authority in
		the EU or UK.
	</p>

	<h2>Children</h2>
	<p>
		noura isn't meant for children under 16, and we don't knowingly collect
		their personal information. If you think a child has given us information,
		contact us and we'll delete it.
	</p>

	<h2>Changes to this policy</h2>
	<p>
		We'll post updates on this page and change the effective date at the top.
	</p>
</LegalPage>

<style>
	code {
		font-size: 14px;
	}
</style>
