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
		on your device. We don't see them, we don't run analytics in the app, and we
		don't sell or rent personal information.
	</p>

	<h2>Who we are</h2>
	<p>
		{legal.entity} ("noura", "we", "us") publishes the noura website, the noura desktop
		app, and the optional noura Sync service. Our mailing address is {legal.address}.
	</p>
	<p>
		{legal.privacyOfficer} is responsible for protecting personal information. Reach
		them at <a href={`mailto:${legal.contactEmail}`}>{legal.contactEmail}</a>.
	</p>

	<h2>What this policy covers</h2>
	<ul>
		<li>The website at noura.app</li>
		<li>The noura desktop app for macOS, Windows, and Linux</li>
		<li>
			noura Sync, an optional service that stores an encrypted copy of your
			workspace so you can use it on several devices
		</li>
	</ul>

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
		request details. The app checks for updates when it starts and every 6
		hours. It sends no account, workspace, or usage data with that check. GitHub
		handles this information under the
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
			<strong>AI features.</strong> If you connect an AI provider, noura sends the
			content you choose directly from your device to that provider, and only after
			you approve it. The provider's own privacy policy applies. We don't receive
			that content.
		</li>
		<li>
			<strong>API keys.</strong> noura stores provider keys in your operating system's
			credential store. They never reach us.
		</li>
		<li>
			<strong>External tools.</strong> Tools you connect through the Model Context
			Protocol read and change your workspace on your device, under your control.
		</li>
	</ul>

	<h3>When you use noura Sync</h3>
	<p>
		Sync is optional and currently available by invitation. Your device encrypts
		your workspace before it uploads anything, and we don't have the keys. We
		can't read your notes, tasks, files, or file names. To run the service, we
		store:
	</p>
	<table>
		<thead>
			<tr><th>Information</th><th>Why we need it</th></tr>
		</thead>
		<tbody>
			<tr>
				<td>Your email address</td>
				<td>To create your account and send sign-in links and invitations</td>
			</tr>
			<tr>
				<td>Passkey public keys and device records</td>
				<td>To sign you in and let you approve new devices</td>
			</tr>
			<tr>
				<td>Session records, including IP address and browser or device type</td
				>
				<td>To keep you signed in, spot suspicious sign-ins, and limit abuse</td
				>
			</tr>
			<tr>
				<td>Encrypted workspace data</td>
				<td>To sync your workspace between your devices</td>
			</tr>
			<tr>
				<td>
					Technical metadata: data sizes, timestamps, share links you create,
					and which devices and accounts share a workspace
				</td>
				<td
					>To deliver changes to the right devices and enforce storage limits</td
				>
			</tr>
		</tbody>
	</table>

	<h2>How we use information</h2>
	<p>We use personal information to:</p>
	<ul>
		<li>provide, secure, and fix the website, the app, and noura Sync</li>
		<li>send messages you ask for, such as sign-in links and invitations</li>
		<li>prevent fraud and abuse</li>
		<li>meet our legal obligations</li>
	</ul>
	<p>
		We don't use your information for advertising, and we don't build profiles
		about you.
	</p>
	<p>
		If the GDPR applies to you, we rely on these legal bases: performing our
		contract with you (running Sync), our legitimate interest in keeping our
		services secure (logs and abuse prevention), and legal obligations.
	</p>

	<h2>Who we share it with</h2>
	<p>We share personal information only with:</p>
	<ul>
		<li>
			service providers that run parts of noura for us: {legal.websiteHost} (website
			hosting), {legal.syncHost} (Sync hosting), {legal.emailProvider} (email delivery),
			and GitHub (downloads and updates)
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
		country, including in the United States. When that happens, we use contracts
		and safeguards that protect your information as this policy describes.
	</p>

	<h2>How long we keep it</h2>
	<ul>
		<li>Server logs: [retention period, for example 30 days]</li>
		<li>Sign-in sessions: 7 days, or until you sign out</li>
		<li>
			Sync account and encrypted data: until you delete your account. We then
			delete it within [deletion period, for example 30 days], except where the
			law requires us to keep it.
		</li>
	</ul>

	<h2>How we protect it</h2>
	<p>
		noura Sync encrypts your workspace end to end, uses encrypted connections,
		and keeps your keys on your devices. We limit staff access to the systems
		that hold personal information. No system is perfectly secure. If a breach
		creates a risk of serious harm, we'll notify you and the relevant
		authorities as the law requires.
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
		We'll post updates on this page and change the effective date. If a change
		affects how we use your information in a significant way, we'll tell Sync
		users by email before it takes effect.
	</p>
</LegalPage>
