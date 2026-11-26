<script lang="ts">
	import AppPart, { type PartName } from '$lib/components/AppPart.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import ProductShowcase from '$lib/components/ProductShowcase.svelte';
	import ShaderBackground from '$lib/components/ShaderBackground.svelte';
	import { onMount } from 'svelte';
	import { detectPlatform, siteConfig, type Platform } from '$lib/site-config';

	const title = 'noura: the open workspace for humans and AI';
	const description =
		'Notes, tasks, projects, and AI in one free, open-source desktop app. noura saves your work as plain Markdown files in a folder you own.';
	const ogImage = `${siteConfig.marketingUrl}og.png`;
	const ogImageAlt =
		'The noura app with the Website relaunch note open in the Markdown editor';

	const features: ReadonlyArray<{
		id: string;
		label: string;
		title: string;
		body: string;
		parts: ReadonlyArray<PartName>;
	}> = [
		{
			id: 'organize',
			label: 'Organize',
			title: 'Notes, tasks, and projects in one place',
			body: 'Link a note to the tasks it creates, and plan your week on the calendar.',
			parts: ['calendar-day', 'task-rows'],
		},
		{
			id: 'own',
			label: 'Own',
			title: 'Plain files you can take anywhere',
			body: 'noura saves each note, task, and project as a Markdown file in a folder you choose. Back it up with the tools you already use, or open it in another editor.',
			parts: ['file-tree'],
		},
		{
			id: 'find',
			label: 'Find',
			title: 'Search names and contents',
			body: 'Type a word to see the files named after it and the notes and tasks that mention it.',
			parts: ['palette'],
		},
		{
			id: 'extend',
			label: 'Extend',
			title: 'Plugins and AI on your terms',
			body: 'Turn features on or off for each workspace. Chat with your own AI provider, or connect external AI tools through MCP.',
			parts: ['mcp'],
		},
	];

	const aiPoints = [
		{
			title: 'Your provider',
			body: 'Connect the AI service you already use. noura keeps your API key in your system keychain.',
		},
		{
			title: 'Your permission',
			body: 'The AI sees workspace content only after you allow it for that provider.',
		},
		{
			title: 'Your approval',
			body: 'noura asks before each tool the AI runs, whether it reads a note or creates a task.',
		},
	] as const;

	const proofs = [
		[
			'Ordinary files',
			'Open your workspace in any text editor, today or ten years from now.',
		],
		[
			'Works offline',
			'Read and edit everything without an internet connection.',
		],
		['No account', 'Install the app and start writing. No sign-up, no email.'],
		[
			'Open source',
			'Read the code, change it, and run your own build. MIT licensed.',
		],
		[
			'Rebuildable index',
			'Delete the search index and noura rebuilds it from your files.',
		],
		[
			'No tracking',
			'The app sends us no analytics or crash reports. The website runs no analytics either.',
		],
	] as const;

	const faqs = [
		[
			'What is noura?',
			'noura is a desktop app for notes, tasks, projects, and your calendar, with AI chat built in. It saves everything as Markdown files in a folder you choose.',
		],
		[
			'Is noura free?',
			'Yes. The app is free, and the source code is open under the MIT license.',
		],
		[
			'Where does my data live?',
			'In a folder on your computer. noura also keeps a local search index for speed. You can delete that index at any time, and noura rebuilds it from your files.',
		],
		[
			'How do updates work?',
			'noura checks for a new version in the background and downloads it for you. When it is ready, you choose when to restart. noura saves your open drafts first, and every update is signed so your computer only installs releases from us. The Linux .deb package is the exception: it doesn’t update itself, so install new versions from this page.',
		],
		[
			'Can I edit my files outside noura?',
			'Yes. Edit, move, or rename files with any app. noura picks up the changes and asks you to review when two edits overlap.',
		],
		[
			'Can AI read my workspace?',
			'The AI chat in noura sees workspace content only after you allow it for your provider, and it asks before each tool the AI runs. AI assistants you connect through MCP work differently: noura doesn’t ask before they read or change your workspace, so connect only assistants you trust.',
		],
		[
			'Do I need an account?',
			'No. noura works offline, and there’s nothing to sign up for.',
		],
		[
			'Which platforms does noura support?',
			'noura runs on macOS, Windows, and Linux. Download the installer for your computer from this page.',
		],
	] as const;

	const sourceCommands = `git clone ${siteConfig.githubUrl}.git
cd noura && rustup target add wasm32-wasip2
scripts/build-shipped-plugins.sh
cargo run -p desktop`;

	const installAnchor = (name: Platform) => `download-${name.toLowerCase()}`;

	let platform = $state<Platform | null>(null);
	const primaryDownload = $derived(
		siteConfig.downloads.find((download) => download.platform === platform),
	);

	onMount(() => {
		platform = detectPlatform();
	});

	let copied = $state(false);
	let copyTimer: ReturnType<typeof setTimeout> | undefined;

	async function copyCommands() {
		try {
			await navigator.clipboard.writeText(sourceCommands);
			copied = true;
			clearTimeout(copyTimer);
			copyTimer = setTimeout(() => (copied = false), 1800);
		} catch {
			copied = false;
		}
	}
</script>

<svelte:head>
	<title>{title}</title>
	<meta name="description" content={description} />
	<link rel="canonical" href={siteConfig.marketingUrl} />
	<meta property="og:title" content={title} />
	<meta property="og:description" content={description} />
	<meta property="og:type" content="website" />
	<meta property="og:site_name" content="noura" />
	<meta property="og:url" content={siteConfig.marketingUrl} />
	<meta property="og:image" content={ogImage} />
	<meta property="og:image:width" content="1200" />
	<meta property="og:image:height" content="630" />
	<meta property="og:image:alt" content={ogImageAlt} />
	<meta name="twitter:card" content="summary_large_image" />
	<meta name="twitter:title" content={title} />
	<meta name="twitter:description" content={description} />
	<meta name="twitter:image" content={ogImage} />
	<meta name="twitter:image:alt" content={ogImageAlt} />
</svelte:head>

<main id="main">
	<section class="dark-world" id="top" aria-labelledby="hero-heading">
		<ShaderBackground />
		<div class="hero">
			<h1 id="hero-heading">
				The open workspace <em>for humans and AI.</em>
			</h1>
			<p class="hero-copy">
				Write notes, plan tasks, and work with AI in one app. noura saves
				everything as plain Markdown files on your computer, so your work stays
				yours.
			</p>
			<div class="hero-actions">
				{#if primaryDownload}
					<a class="pill pill-light pill-lg" href={primaryDownload.url}>
						Download for {primaryDownload.platform}
					</a>
				{:else}
					<a class="pill pill-light pill-lg" href="#download">Download noura</a>
				{/if}
				<a class="pill pill-ghost pill-lg" href={siteConfig.githubUrl}>
					<Icon name="github" size={20} />Star on GitHub
				</a>
			</div>
			<p class="hero-note">
				{#if primaryDownload?.installNote}
					{primaryDownload.platform} may warn you the first time you open noura.
					<a href={`#${installAnchor(primaryDownload.platform)}`}>What to do</a>
				{/if}
			</p>
		</div>

		<div class="hero-visual">
			<ProductShowcase />
		</div>

		<div class="open-copy" id="features">
			<div>
				<h2>Your work stays open.</h2>
				<p>
					noura gives you a fast, focused workspace. Underneath, your files stay
					readable in any text editor, with or without noura.
				</p>
			</div>
			<a class="open-cta" href="#how-it-works">How it works</a>
		</div>

		<div class="feature-grid">
			{#each features as feature (feature.id)}
				<article class={['feature-card', `feature-${feature.id}`]}>
					<span class="feature-label">{feature.label}</span>
					<h3>{feature.title}</h3>
					<p>{feature.body}</p>
					<div class="feature-visual">
						{#each feature.parts as part (part)}
							<div class={['feature-part', `part-${part}`]}>
								<AppPart name={part} theme="dark" />
							</div>
						{/each}
					</div>
				</article>
			{/each}
		</div>
	</section>

	<section
		class="product-world"
		id="how-it-works"
		aria-labelledby="product-heading"
	>
		<div class="section-intro">
			<p class="kicker">Built from plugins</p>
			<h2 id="product-heading">
				Turn on what you need. <em>Leave out the rest.</em>
			</h2>
			<p>
				Your files and the Markdown editor are always there. Tasks, projects,
				calendar, and AI are plugins: pick the ones each workspace needs.
				Turning one off hides it and leaves its files where they are.
			</p>
		</div>

		<div class="plugins-stage">
			<AppPart name="plugins" theme="light" fluid />
		</div>

		<div class="product-followup">
			<div>
				<p class="kicker">Built on files</p>
				<h3>Your folder is the database.</h3>
			</div>
			<p>
				noura keeps a local index so search stays fast. Delete it, and noura
				rebuilds it from your files. Edit a file in another app, and noura shows
				the change right away.
			</p>
		</div>
	</section>

	<section class="ai" id="ai" aria-labelledby="ai-heading">
		<div class="ai-copy">
			<p class="kicker">AI with permission</p>
			<h2 id="ai-heading">AI that asks before it acts.</h2>
			<p class="ai-lede">
				Ask questions about your notes, turn plans into tasks, and summarize
				projects. You decide what the AI sees and what it can change.
			</p>
			<ul class="ai-points">
				{#each aiPoints as point, index (point.title)}
					<li>
						<span>0{index + 1}</span>
						<div>
							<h3>{point.title}</h3>
							<p>{point.body}</p>
						</div>
					</li>
				{/each}
			</ul>
			<p class="mcp-note">
				<Icon name="sparkle" size={16} />
				<span>
					Already use an AI assistant? Connect it through the
					<a href="https://modelcontextprotocol.io/">Model Context Protocol</a>
					and let it read and update your notes and tasks. noura doesn't ask before
					each of its actions, so connect only assistants you trust.
				</span>
			</p>
		</div>
		<div class="ai-visual">
			<div class="ai-part">
				<AppPart name="ai-dialog" theme="light" />
			</div>
		</div>
	</section>

	<section class="trust" id="privacy" aria-labelledby="trust-heading">
		<div class="trust-intro">
			<p class="kicker">Privacy</p>
			<h2 id="trust-heading">Yours by default.</h2>
			<p>
				noura runs on your computer and saves your work as ordinary files. You
				don't need an account, and anyone can read the code.
			</p>
		</div>
		<div class="proof-grid">
			{#each proofs as proof, index (proof[0])}
				<article>
					<span>0{index + 1}</span>
					<h3>{proof[0]}</h3>
					<p>{proof[1]}</p>
				</article>
			{/each}
		</div>
	</section>

	<section class="faq" id="faq" aria-labelledby="faq-heading">
		<div>
			<p class="kicker">FAQ</p>
			<h2 id="faq-heading">Questions, answered.</h2>
			<p class="faq-more">
				Something else? <a href={`${siteConfig.githubUrl}/issues`}
					>Ask on GitHub</a
				>.
			</p>
		</div>
		<div class="faq-list">
			{#each faqs as faq, index (faq[0])}
				<details open={index === 0}>
					<summary>{faq[0]}<span aria-hidden="true">+</span></summary>
					<p>{faq[1]}</p>
				</details>
			{/each}
		</div>
	</section>

	<section class="download" id="download" aria-labelledby="download-heading">
		<div class="download-inner">
			<div class="download-intro">
				<p class="kicker">Download</p>
				<h2 id="download-heading">
					Start with a folder. <em>Keep it forever.</em>
				</h2>
				<p>
					noura is free and open source. Install it once, and it updates itself
					when a new version ships.
				</p>
				<p class="alpha-note">Early alpha. Keep backups of important files.</p>
			</div>

			<div class="download-grid">
				<div class="platform-list">
					{#each siteConfig.downloads as download (download.platform)}
						<div
							class="platform-row"
							class:detected={download.platform === platform}
							id={installAnchor(download.platform)}
						>
							<div>
								<h3>
									{download.platform}
									{#if download.platform === platform}
										<small>Your computer</small>
									{/if}
								</h3>
								<p>
									{download.detail}
									{#if download.alternate}
										· <a href={download.alternate.url}
											>{download.alternate.label}</a
										>
									{/if}
								</p>
								{#if download.installNote}
									<p class="install-note">{download.installNote}</p>
								{/if}
							</div>
							<a class="pill pill-light" href={download.url}>Download</a>
						</div>
					{/each}
					<p class="update-note">
						<Icon name="check" size={16} />
						<span>
							Signed updates download in the background, except with the .deb
							package.
							<a href={siteConfig.releasesUrl}>See all releases</a>.
						</span>
					</p>
				</div>

				<div class="source-card">
					<div class="source-head">
						<span>Build from source</span>
						<button type="button" onclick={copyCommands}>
							{copied ? 'Copied' : 'Copy'}
						</button>
					</div>
					<pre><code>{sourceCommands}</code></pre>
					<p>
						Requires rustup, plus NASM on Intel and AMD.
						<a href={`${siteConfig.githubUrl}#run-noura-from-source`}
							>Read the setup guide</a
						>.
					</p>
					<span class="visually-hidden" aria-live="polite">
						{copied ? 'Commands copied to clipboard' : ''}
					</span>
				</div>
			</div>
		</div>
	</section>
</main>

<style>
	/* Buttons */
	.pill {
		display: inline-flex;
		min-height: 40px;
		align-items: center;
		justify-content: center;
		gap: 8px;
		border-radius: 999px;
		padding: 0 20px;
		text-decoration: none;
		font-size: 14px;
		font-weight: 700;
		white-space: nowrap;
		transition:
			transform 180ms ease,
			background 180ms ease;
	}
	.pill:hover {
		transform: translateY(-2px);
	}
	.pill-lg {
		min-height: 48px;
		padding: 0 26px;
		font-size: 15px;
	}
	.pill-light {
		background: #f6f7f4;
		color: #111114;
	}
	.pill-ghost {
		background: rgba(255, 255, 255, 0.1);
		color: #f4f4f5;
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
	}
	.pill-ghost:hover {
		background: rgba(255, 255, 255, 0.15);
	}

	/* Shared type */
	h1,
	h2,
	h3,
	p {
		margin-top: 0;
	}
	em {
		font-style: normal;
		background: linear-gradient(
			134deg,
			#e5e5ff 0%,
			#9391fe 49.47%,
			#e5e5ff 91.25%
		);
		background-clip: text;
		-webkit-background-clip: text;
		color: transparent;
	}
	.product-world em {
		background: linear-gradient(134deg, #7d7af0 0%, #5d5ad6 50%, #8f8cff 100%);
		background-clip: text;
		-webkit-background-clip: text;
	}
	.kicker {
		margin: 0 0 16px;
		color: #6663d8;
		font-size: 14px;
		font-weight: 720;
	}

	/* Hero */
	.dark-world {
		position: relative;
		isolation: isolate;
		margin: 0 24px 24px;
		padding: 88px 48px 80px;
		border-radius: 32px;
		overflow: hidden;
		color: var(--chalk);
		background:
			radial-gradient(
				circle at 48% 64%,
				rgba(147, 145, 254, 0.15),
				transparent 34%
			),
			linear-gradient(180deg, #0f0f10 0%, #0f0f10 54%, #1c1725 100%);
	}
	.hero,
	.hero-visual,
	.open-copy,
	.feature-grid {
		position: relative;
		z-index: 1;
	}
	.hero {
		max-width: 986px;
		margin: 0 auto;
		text-align: center;
	}
	h1 {
		max-width: 900px;
		margin: 0 auto 22px;
		font-size: clamp(48px, 5.6vw, 80px);
		line-height: 1.05;
		letter-spacing: -0.05em;
		font-weight: 560;
		text-wrap: balance;
	}
	.hero-copy {
		max-width: 620px;
		margin: 0 auto;
		color: #b3b1b8;
		font-size: 18px;
		line-height: 1.6;
		text-wrap: pretty;
	}
	.hero-actions {
		display: flex;
		justify-content: center;
		gap: 12px;
		margin-top: 34px;
	}
	.hero-note {
		/* Holds its line before the platform is known, so nothing below moves. */
		min-height: 21px;
		margin: 18px 0 0;
		color: #8f8d95;
		font-size: 14px;
		line-height: 1.5;
	}
	.hero-note a {
		color: #c4c2ff;
		text-underline-offset: 3px;
	}
	.hero-visual {
		max-width: 1120px;
		margin: 56px auto 0;
	}

	/* Open files + features */
	.open-copy {
		max-width: 1320px;
		margin: 150px auto 64px;
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: 48px;
		align-items: end;
	}
	.open-copy h2 {
		margin: 0 0 20px;
		font-size: clamp(40px, 3.6vw, 56px);
		line-height: 1.08;
		letter-spacing: -0.04em;
		font-weight: 560;
	}
	.open-copy p {
		max-width: 720px;
		margin: 0;
		color: #aaa7af;
		font-size: clamp(18px, 1.35vw, 21px);
		line-height: 1.5;
	}
	.open-cta {
		display: inline-flex;
		min-height: 56px;
		align-items: center;
		padding: 0 30px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.1);
		color: #f6f4f8;
		text-decoration: none;
		font-size: 16px;
		font-weight: 650;
		transition:
			transform 180ms ease,
			background 180ms ease;
	}
	.open-cta:hover {
		transform: translateY(-2px);
		background: rgba(255, 255, 255, 0.14);
	}
	.feature-grid {
		max-width: 1320px;
		margin: 0 auto;
		display: grid;
		grid-template-columns: repeat(12, minmax(0, 1fr));
		gap: 24px;
	}
	.feature-card {
		position: relative;
		isolation: isolate;
		display: flex;
		flex-direction: column;
		grid-column: span 5;
		overflow: hidden;
		padding: 34px 32px 0;
		border-radius: 28px;
		background: rgba(43, 36, 51, 0.72);
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.05);
	}
	.feature-organize,
	.feature-extend {
		grid-column: span 7;
	}
	.feature-label {
		color: #a39cff;
		font-size: 15px;
		font-weight: 700;
	}
	.feature-card h3 {
		margin: 14px 0 16px;
		font-size: 27px;
		line-height: 1.15;
		letter-spacing: -0.035em;
		font-weight: 580;
		text-wrap: balance;
	}
	.feature-card p {
		max-width: 480px;
		margin: 0 0 40px;
		color: #aaa7af;
		font-size: 17px;
		line-height: 1.55;
		text-wrap: pretty;
	}
	/* The app's own interface, sitting in the card's lower part and cut off by its edges. */
	.feature-visual {
		position: relative;
		flex: 1 0 320px;
		margin: 0 -32px;
	}
	.feature-part {
		position: absolute;
		top: 0;
		left: 32px;
	}
	.part-task-rows {
		top: 136px;
		right: 32px;
		left: auto;
		z-index: 1;
	}
	.part-file-tree {
		left: 50%;
		transform: translateX(-50%);
	}

	/* Plugins */
	.product-world {
		margin: 0 24px;
		border-radius: 32px;
		overflow: hidden;
		padding: 118px 48px 64px;
		background:
			radial-gradient(
				ellipse 55% 40% at 50% 52%,
				rgba(147, 145, 254, 0.42),
				transparent 70%
			),
			linear-gradient(180deg, #fff 4%, #f2f0ff 48%, #dedcff 100%);
	}
	.section-intro {
		max-width: 840px;
		margin: 0 auto;
		text-align: center;
	}
	.section-intro h2 {
		margin: 0 auto 20px;
		font-size: clamp(42px, 5vw, 68px);
		line-height: 1.07;
		letter-spacing: -0.05em;
		font-weight: 560;
		text-wrap: balance;
	}
	.section-intro > p:last-child {
		max-width: 600px;
		margin: 0 auto;
		color: #5f5d68;
		font-size: 18px;
		line-height: 1.6;
		text-wrap: pretty;
	}
	.plugins-stage {
		display: flex;
		justify-content: center;
		margin: 72px auto 0;
	}
	.product-followup {
		max-width: 1120px;
		margin: 96px auto 0;
		display: grid;
		grid-template-columns: 1.1fr 0.9fr;
		gap: 70px;
		align-items: end;
	}
	.product-followup .kicker {
		margin-bottom: 12px;
	}
	.product-followup h3 {
		margin-bottom: 0;
		font-size: clamp(34px, 4vw, 52px);
		line-height: 1.08;
		letter-spacing: -0.045em;
		font-weight: 540;
		text-wrap: balance;
	}
	.product-followup > p {
		max-width: 500px;
		margin: 0;
		color: #5f5d68;
		font-size: 17px;
		line-height: 1.6;
	}

	/* Content sections */
	.ai,
	.trust,
	.faq {
		max-width: 1320px;
		margin: 0 auto;
		padding: 150px 48px 0;
	}
	.ai {
		display: grid;
		grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.1fr);
		gap: 72px;
	}
	/* The approval prompt floats in the middle, or runs off the right edge when it doesn't fit. */
	.ai-visual {
		display: flex;
		align-self: center;
		align-items: center;
		justify-content: safe center;
		overflow: hidden;
		padding: 88px 48px 96px;
		border-radius: 32px;
		background:
			radial-gradient(
				circle at 50% 50%,
				rgba(147, 145, 254, 0.42),
				transparent 62%
			),
			linear-gradient(180deg, #f4f2ff, #e2e0ff);
	}
	.ai-part {
		flex: none;
	}
	.ai h2,
	.trust-intro h2,
	.faq h2,
	.download h2 {
		margin: 0;
		font-size: clamp(40px, 4.6vw, 62px);
		line-height: 1.05;
		letter-spacing: -0.05em;
		font-weight: 560;
		text-wrap: balance;
	}
	.ai-lede {
		max-width: 630px;
		margin: 22px 0 0;
		color: #5f5d68;
		font-size: 18px;
		line-height: 1.6;
	}
	.ai-points {
		margin: 36px 0 0;
		padding: 0;
		list-style: none;
		border-top: 1px solid var(--line);
	}
	.ai-points li {
		display: grid;
		grid-template-columns: 40px 1fr;
		padding: 20px 0;
		border-bottom: 1px solid var(--line);
	}
	.ai-points li > span {
		padding-top: 3px;
		color: #6663d8;
		font-size: 13px;
		font-weight: 780;
	}
	.ai-points h3 {
		margin: 0 0 4px;
		font-size: 18px;
		letter-spacing: -0.02em;
	}
	.ai-points p {
		margin: 0;
		color: #6f6d76;
		font-size: 15px;
		line-height: 1.55;
	}
	.mcp-note {
		display: flex;
		gap: 12px;
		margin: 28px 0 0;
		padding: 16px 18px;
		border-radius: 14px;
		background: var(--mist);
		color: #4b4a52;
		font-size: 14.5px;
		line-height: 1.55;
	}
	.mcp-note :global(svg) {
		margin-top: 3px;
		color: #6663d8;
	}
	.mcp-note a {
		color: #4f4cc4;
	}

	/* Privacy */
	.trust-intro {
		max-width: 720px;
	}
	.trust-intro > p:last-child {
		max-width: 630px;
		margin: 22px 0 0;
		color: #5f5d68;
		font-size: 18px;
		line-height: 1.6;
	}
	.proof-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		margin-top: 64px;
		border-top: 1px solid var(--line);
		border-left: 1px solid var(--line);
	}
	.proof-grid article {
		min-height: 200px;
		padding: 28px;
		border-right: 1px solid var(--line);
		border-bottom: 1px solid var(--line);
	}
	.proof-grid span {
		color: var(--lavender);
		font-size: 13px;
		font-weight: 780;
	}
	.proof-grid h3 {
		margin: 36px 0 8px;
		font-size: 22px;
		letter-spacing: -0.035em;
	}
	.proof-grid p {
		max-width: 340px;
		margin: 0;
		color: #6f6d76;
		font-size: 15px;
		line-height: 1.55;
	}

	/* FAQ */
	.faq {
		display: grid;
		grid-template-columns: 0.75fr 1.25fr;
		gap: 80px;
		padding-bottom: 150px;
	}
	.faq-more {
		margin: 20px 0 0;
		color: #6f6d76;
		font-size: 15px;
	}
	.faq-more a {
		color: #4f4cc4;
		font-weight: 650;
	}
	.faq-list {
		border-top: 1px solid var(--line);
	}
	details {
		border-bottom: 1px solid var(--line);
		padding: 0 4px;
	}
	summary {
		min-height: 68px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 20px;
		cursor: pointer;
		list-style: none;
		font-size: 17px;
		font-weight: 650;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary span {
		color: var(--lavender);
		font-size: 24px;
		font-weight: 400;
		transition: transform 180ms ease;
	}
	details[open] summary span {
		transform: rotate(45deg);
	}
	details p {
		max-width: 660px;
		margin: -4px 38px 24px 0;
		color: #5f5d68;
		font-size: 16px;
		line-height: 1.65;
	}

	/* Download */
	.download {
		margin: 0 24px 24px;
		border-radius: 32px;
		background:
			radial-gradient(
				circle at 85% 0%,
				rgba(147, 145, 254, 0.28),
				transparent 45%
			),
			linear-gradient(180deg, #121114, #1c1725);
		color: var(--chalk);
	}
	.download-inner {
		max-width: 1320px;
		margin: 0 auto;
		padding: 110px 48px 96px;
	}
	.download .kicker {
		color: #a39cff;
	}
	.download-intro {
		max-width: 760px;
	}
	.download-intro > p:not(.kicker) {
		max-width: 600px;
		margin: 22px 0 0;
		color: #aaa7af;
		font-size: 18px;
		line-height: 1.6;
	}
	.download-intro > p.alpha-note {
		display: inline-block;
		margin-top: 18px;
		padding: 6px 14px;
		border-radius: 999px;
		background: rgba(147, 145, 254, 0.16);
		color: #d7d6ff;
		font-size: 14px;
		font-weight: 650;
	}
	.download-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 24px;
		margin-top: 56px;
	}
	.download-grid > * {
		min-width: 0;
	}
	.platform-list {
		display: flex;
		flex-direction: column;
		padding: 8px 28px 28px;
		border-radius: 24px;
		background: rgba(255, 255, 255, 0.05);
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
	}
	.platform-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		min-height: 84px;
		padding: 16px 0;
		border-bottom: 1px solid rgba(255, 255, 255, 0.08);
	}
	.platform-row h3 {
		margin: 0 0 2px;
		font-size: 19px;
		letter-spacing: -0.02em;
	}
	.platform-row p {
		margin: 0;
		color: #8f8d95;
		font-size: 14px;
	}
	.platform-row.detected h3 small {
		margin-left: 8px;
		padding: 2px 8px;
		border-radius: 999px;
		background: rgba(147, 145, 254, 0.2);
		color: #c4c2ff;
		font-size: 12px;
		font-weight: 650;
		letter-spacing: 0;
		vertical-align: 2px;
	}
	.platform-row p.install-note {
		max-width: 420px;
		margin-top: 6px;
		color: #aaa7af;
		font-size: 13px;
		line-height: 1.5;
	}
	.platform-row p a {
		color: #c4c2ff;
	}
	.update-note {
		display: flex;
		align-items: center;
		gap: 10px;
		margin: 22px 0 0;
		color: #aaa7af;
		font-size: 14px;
	}
	.update-note :global(svg) {
		color: var(--lavender);
	}
	.update-note a {
		color: #c4c2ff;
	}
	.source-card {
		display: flex;
		flex-direction: column;
		padding: 24px 28px 28px;
		border-radius: 24px;
		background: #0c0c0e;
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
	}
	.source-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		font-size: 15px;
		font-weight: 700;
	}
	.source-head button {
		min-height: 32px;
		padding: 0 14px;
		border: 0;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.1);
		color: #e9e8ec;
		font-size: 13px;
		font-weight: 650;
		cursor: pointer;
	}
	.source-head button:hover {
		background: rgba(255, 255, 255, 0.16);
	}
	pre {
		flex: 1;
		margin: 18px 0;
		padding: 20px;
		overflow-x: auto;
		border-radius: 14px;
		background: #17171a;
		color: #d7d6ff;
		font-family: ui-monospace, 'SF Mono', Menlo, monospace;
		font-size: 14px;
		line-height: 1.8;
	}
	.source-card > p {
		margin: 0;
		color: #8f8d95;
		font-size: 14px;
		line-height: 1.55;
	}
	.source-card a {
		color: #c4c2ff;
	}
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
		white-space: nowrap;
	}

	/* Responsive */
	@media (max-width: 1080px) {
		.ai {
			grid-template-columns: 1fr;
			gap: 48px;
		}
		.part-task-rows {
			top: 176px;
		}
	}
	@media (max-width: 900px) {
		.dark-world,
		.product-world,
		.download {
			margin-left: 10px;
			margin-right: 10px;
			border-radius: 24px;
		}
		.dark-world,
		.product-world {
			padding-left: 24px;
			padding-right: 24px;
		}
		.dark-world {
			padding-top: 64px;
		}
		.open-copy,
		.product-followup {
			grid-template-columns: 1fr;
			gap: 24px;
		}
		.open-copy {
			margin-top: 110px;
		}
		.open-cta {
			justify-self: start;
		}
		.feature-grid {
			grid-template-columns: 1fr;
		}
		.feature-card,
		.feature-organize,
		.feature-extend {
			grid-column: auto;
		}
		.proof-grid {
			grid-template-columns: 1fr 1fr;
		}
		.faq {
			grid-template-columns: 1fr;
			gap: 32px;
		}
		.download-grid {
			grid-template-columns: 1fr;
		}
		.download-inner {
			padding: 84px 24px 64px;
		}
	}
	@media (max-width: 720px) {
		.ai-visual {
			--part-scale: 0.8;
			padding: 64px 24px 72px;
		}
	}
	@media (max-width: 600px) {
		.pill {
			min-height: 38px;
			padding: 0 16px;
			font-size: 13px;
		}
		.pill-lg {
			min-height: 46px;
			padding: 0 22px;
			font-size: 14px;
		}
		.dark-world {
			padding: 48px 16px 28px;
		}
		h1 {
			font-size: 44px;
		}
		.hero-copy {
			font-size: 16px;
		}
		.hero-actions {
			flex-direction: column;
			align-items: stretch;
			max-width: 320px;
			margin-left: auto;
			margin-right: auto;
		}
		.hero-note {
			max-width: 320px;
			margin-left: auto;
			margin-right: auto;
			font-size: 13px;
		}
		.hero-visual {
			margin-top: 44px;
		}
		.open-copy {
			margin: 84px auto 40px;
		}
		.feature-card {
			padding: 28px 24px 0;
		}
		.feature-card h3 {
			font-size: 24px;
		}
		.feature-card p {
			margin-bottom: 32px;
			font-size: 16px;
		}
		.feature-visual {
			--part-scale: 0.8;
			flex-basis: 260px;
			margin: 0 -24px;
		}
		.feature-part {
			left: 24px;
		}
		.part-task-rows {
			top: 112px;
			right: auto;
			left: 64px;
		}
		.part-file-tree {
			left: 50%;
		}
		.product-world {
			padding: 84px 16px 48px;
		}
		.section-intro > p:last-child {
			font-size: 16px;
		}
		.plugins-stage {
			margin: 48px -6px 0;
		}
		.product-followup {
			margin-top: 64px;
		}
		.ai-visual {
			/* The dialog's transparent shadow margin can sit in the padding. */
			--part-scale: 0.64;
			padding: 40px 8px 48px;
			border-radius: 24px;
		}
		.ai,
		.trust,
		.faq {
			padding-left: 20px;
			padding-right: 20px;
			padding-top: 100px;
		}
		.proof-grid {
			grid-template-columns: 1fr;
		}
		.proof-grid article {
			min-height: 0;
		}
		.proof-grid h3 {
			margin-top: 20px;
		}
		.faq {
			padding-bottom: 100px;
		}
		summary {
			font-size: 16px;
		}
		.platform-list,
		.source-card {
			padding-left: 20px;
			padding-right: 20px;
		}
		pre {
			font-size: 12.5px;
		}
	}
</style>
