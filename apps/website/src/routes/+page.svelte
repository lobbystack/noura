<script lang="ts">
	import ShaderBackground from '$lib/components/ShaderBackground.svelte';
	import { siteConfig } from '$lib/site-config';

	const features = [
		{
			label: 'Organize',
			title: 'One workspace for your work and knowledge',
			body: 'Notes, tasks, projects, calendar, and search share the same open workspace.',
			chips: ['Notes', 'Tasks', 'Projects', 'Calendar'],
		},
		{
			label: 'Own',
			title: 'Your files remain the source of truth',
			body: 'Durable workspace data lives in ordinary files you can inspect, edit, move, and back up.',
			chips: ['Local-first', 'Open files', 'Offline', 'Portable'],
		},
		{
			label: 'Extend',
			title: 'Built for humans, software, and AI',
			body: 'Use plugins and authorized AI tools without locking your knowledge inside one application.',
			chips: ['Plugins', 'AI', 'MCP', 'Open source'],
		},
	] as const;

	const proofs = [
		['Ordinary files', 'Your workspace remains useful without Noura.'],
		['Local-first', 'Core work stays available without a connection.'],
		['No account', 'Open the app and get to work.'],
		['Open source', 'Inspect, adapt, and self-host the code.'],
		[
			'Rebuildable index',
			'Delete the local index and rebuild it from your files.',
		],
		[
			'Encrypted sync',
			'Managed sync is designed so the server cannot read workspace content.',
		],
	] as const;

	const faqs = [
		[
			'What is Noura?',
			'Noura is an open, local-first workspace for humans and AI. It brings notes, tasks, projects, calendar, search, plugins, and AI tools over ordinary files.',
		],
		[
			'Is Noura free and open source?',
			'Yes. The core app is free to use and its source is available under the MIT License.',
		],
		[
			'Where is my data stored?',
			'Durable workspace data lives in ordinary files inside the folder you choose. Noura uses a disposable local SQLite index for speed, and can rebuild it from those files.',
		],
		[
			'Can AI read my workspace?',
			'Only through an explicitly authorized action on a client or device. Sending workspace content to an AI provider requires your authorization.',
		],
		[
			'Do I need Noura Sync?',
			"No. Local functionality does not depend on Noura's cloud. Managed sync is optional and end-to-end encrypted so hosted infrastructure does not need plaintext workspace content.",
		],
	] as const;

	const chart = [58, 72, 64, 88, 79, 46, 34] as const;
	const updates = [42, 61, 52, 73, 69, 31, 24] as const;
</script>

<svelte:head>
	<title>Noura: the open workspace for humans and AI</title>
	<meta
		name="description"
		content="A free, open-source, local-first workspace where ordinary files remain the durable source of truth. Built for notes, tasks, projects, plugins, and authorized AI."
	/>
	<link rel="canonical" href={siteConfig.marketingUrl} />
	<meta
		property="og:title"
		content="Noura: the open workspace for humans and AI"
	/>
	<meta
		property="og:description"
		content="A local-first workspace where humans, software, and authorized AI work with ordinary files."
	/>
	<meta property="og:type" content="website" />
	<meta property="og:url" content={siteConfig.marketingUrl} />
	<link rel="icon" href="/favicon.svg" />
</svelte:head>

<header class="site-header">
	<a class="brand" href="#top" aria-label="Noura home">
		<svg viewBox="0 0 32 32" aria-hidden="true">
			<path d="M6 23V9l10 9 10-9v14" />
		</svg>
		<span>Noura</span>
	</a>
	<nav aria-label="Primary navigation">
		<a href={siteConfig.githubUrl}>GitHub</a>
		<a class="header-download" href="#download">Download</a>
		<a class="pill pill-dark" href="#product-heading">Explore Noura</a>
	</nav>
</header>

<main id="top">
	<section class="dark-world" aria-labelledby="hero-heading">
		<ShaderBackground />
		<div class="hero">
			<h1 id="hero-heading">
				The open workspace <em>for humans and AI.</em>
			</h1>
			<p class="hero-copy">
				Bring notes, tasks, projects, calendar, search, plugins, and AI into one
				local-first workspace whose ordinary files stay yours.
			</p>
			<div class="hero-actions">
				<a class="pill pill-light" href="#download">Download</a>
				<a class="pill pill-ghost" href={siteConfig.githubUrl}>View on GitHub</a
				>
			</div>
			<p class="trust-line">
				Free &amp; open source · Local-first · Ordinary files · Optional
				encrypted sync
			</p>
		</div>

		<div class="demo-shell" id="demo">
			<div class="window-bar" aria-hidden="true">
				<div class="traffic"><i></i><i></i><i></i></div>
				<span class="address-bar">demo.noura.app</span>
			</div>
			<iframe
				title="Interactive Noura demo"
				src={siteConfig.demoUrl}
				sandbox="allow-scripts allow-same-origin allow-forms allow-modals"
				loading="eager"
			></iframe>
		</div>

		<div class="markets-copy">
			<div class="markets-text">
				<h2>Your work stays open.</h2>
				<p>
					Use a polished workspace without giving up the files underneath it.
				</p>
			</div>
			<a class="markets-cta" href="#product-heading">Learn More</a>
		</div>

		<div class="feature-grid">
			{#each features as feature (feature.label)}
				<article class="feature-card">
					<span class="feature-label">{feature.label}</span>
					<h3>{feature.title}</h3>
					<p>{feature.body}</p>
					<div class="feature-footer">
						{#each feature.chips as chip (chip)}
							<span class="feature-chip"><i></i>{chip}</span>
						{/each}
					</div>
				</article>
			{/each}
		</div>
	</section>

	<section class="product-world" aria-labelledby="product-heading">
		<div class="section-intro">
			<h2 id="product-heading">One place for <em>your whole workspace.</em></h2>
			<p>
				Connect knowledge and action while keeping durable state in formats you
				can use outside the application.
			</p>
			<div class="hero-actions">
				<a class="pill pill-lavender" href="#demo">Try the Demo</a>
				<a class="pill pill-outline" href={siteConfig.githubUrl}>View Source</a>
			</div>
		</div>

		<div class="product-stage" aria-label="Noura product views">
			<div class="mini-sidebar">
				<span class="mini-brand">N</span>
				<i class="active"></i><i></i><i></i><i></i>
			</div>
			<div class="mini-panel task-panel">
				<div class="mini-label">WORKSPACE</div>
				<h3>Knowledge meets action.</h3>
				{#each ['Write launch brief', 'Review research notes', 'Plan the next milestone'] as task, index (task)}
					<div class="task-row">
						<i class:done={index === 0}></i><span>{task}</span><small
							>{['Note', 'Research', 'Task'][index]}</small
						>
					</div>
				{/each}
			</div>
			<div class="mini-panel planner-panel">
				<div class="mini-label">CALENDAR</div>
				<div class="time-slot">
					<small>09:00</small><span>Project review</span>
				</div>
				<div class="time-slot lavender">
					<small>10:00</small><span>Launch planning</span>
				</div>
				<div class="time-slot short">
					<small>13:30</small><span>Read notes</span>
				</div>
			</div>
			<div class="focus-orb"><span>.md</span><small>FILES</small></div>
		</div>

		<div class="product-followup">
			<div>
				<p class="kicker">One open foundation</p>
				<h3>Files first. Fast when indexed.</h3>
			</div>
			<p>
				Noura adds structured views, search, plugins, and AI while your
				workspace files remain canonical and portable.
			</p>
		</div>
	</section>

	<section class="platforms" id="download" aria-labelledby="platform-heading">
		<div class="wide-heading">
			<div>
				<p class="kicker">Download</p>
				<h2 id="platform-heading">Works where you do.</h2>
			</div>
			<p>Noura is being built for macOS, Windows, Linux, and mobile devices.</p>
		</div>
		<div class="platform-grid">
			{#each siteConfig.downloads as download (download.platform)}
				<article class="platform-card">
					<span class="platform-icon" aria-hidden="true"
						>{download.platform.slice(0, 1)}</span
					>
					<div>
						<h3>{download.platform}</h3>
						<p>{download.detail}</p>
					</div>
					<span class="soon">Coming soon</span>
				</article>
			{/each}
			<a class="platform-card web-card" href={siteConfig.githubUrl}>
				<span class="platform-icon" aria-hidden="true">G</span>
				<div>
					<h3>Source</h3>
					<p>Follow development on GitHub</p>
				</div>
				<span class="arrow">↗</span>
			</a>
		</div>
	</section>

	<section class="trust" aria-labelledby="trust-heading">
		<div class="trust-intro">
			<h2 id="trust-heading">Yours by Default</h2>
			<p>
				Noura works locally and keeps durable workspace information in ordinary
				files. Its code is open, and an account is never required for core work.
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

	<section class="insights" aria-labelledby="insights-heading">
		<div class="insights-heading">
			<div>
				<p class="kicker">Workspace pulse</p>
				<h2 id="insights-heading">See your work take shape.</h2>
			</div>
			<p>
				Search and structured views turn ordinary workspace files into a useful,
				connected view of what changed.
			</p>
		</div>
		<div class="chart-card">
			<div class="chart-legend">
				<span><i class="planned"></i>Created</span><span
					><i class="focused"></i>Updated</span
				>
			</div>
			<div
				class="chart-plot"
				aria-label="Sample chart comparing created and updated workspace objects from Monday to Sunday"
			>
				{#each chart as height, index (height)}
					<div
						class="day"
						style:--planned={`${height}%`}
						style:--focused={`${updates[index]}%`}
					>
						<div class="bars">
							<i class="planned-bar"></i><i class="focus-bar"></i>
						</div>
						<span>{['M', 'T', 'W', 'T', 'F', 'S', 'S'][index]}</span>
					</div>
				{/each}
			</div>
		</div>
	</section>

	<section class="faq" aria-labelledby="faq-heading">
		<h2 id="faq-heading">FAQs</h2>
		<div class="faq-list">
			{#each faqs as faq, index (faq[0])}
				<details open={index === 0}>
					<summary>{faq[0]}<span>+</span></summary>
					<p>{faq[1]}</p>
				</details>
			{/each}
		</div>
	</section>
</main>

<footer>
	<div class="footer-brand">
		<span>Noura</span>
		<p>The open workspace for humans and AI.</p>
	</div>
	<nav aria-label="Footer navigation">
		<a href={siteConfig.githubUrl}>GitHub</a>
		<a href="#download">Download</a>
		<a href={siteConfig.githubUrl}>Source</a>
		<a href={`${siteConfig.githubUrl}/blob/main/LICENSE`}>MIT License</a>
		<a href={`${siteConfig.githubUrl}/tree/main/docs/security`}>Security</a>
	</nav>
	<p class="copyright">© {new Date().getFullYear()} Noura</p>
</footer>

<style>
	.site-header {
		height: 76px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		max-width: 1440px;
		margin: 0 auto;
		padding: 0 32px;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 9px;
		text-decoration: none;
		font-weight: 760;
		letter-spacing: -0.03em;
	}
	.brand svg {
		width: 28px;
		height: 28px;
	}
	.brand path {
		fill: none;
		stroke: currentColor;
		stroke-width: 3;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.site-header nav {
		display: flex;
		align-items: center;
		gap: 24px;
	}
	.site-header nav a {
		text-decoration: none;
		font-size: 13px;
		font-weight: 650;
	}
	.pill {
		display: inline-flex;
		min-height: 38px;
		align-items: center;
		justify-content: center;
		border-radius: 999px;
		padding: 0 20px;
		text-decoration: none;
		font-size: 13px;
		font-weight: 720;
		transition:
			transform 180ms ease,
			background 180ms ease;
	}
	.pill:hover {
		transform: translateY(-2px);
	}
	.pill-dark {
		background: var(--ink);
		color: white;
	}
	.dark-world {
		position: relative;
		isolation: isolate;
		margin: 0 24px 24px;
		padding: 78px 48px 80px;
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
	.demo-shell,
	.markets-copy,
	.feature-grid {
		position: relative;
		z-index: 1;
	}
	.hero {
		max-width: 986px;
		margin: 0 auto;
		text-align: center;
	}
	h1,
	h2,
	h3,
	p {
		margin-top: 0;
	}
	h1 {
		max-width: 900px;
		margin: 0 auto 20px;
		font-size: clamp(48px, 5vw, 72px);
		line-height: 1.1;
		letter-spacing: -0.05em;
		font-weight: 560;
		text-wrap: balance;
	}
	em {
		font-family: 'DM Serif Display', ui-serif, Georgia, serif;
		font-style: italic;
		font-weight: 400;
		letter-spacing: -0.02em;
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
	.hero-copy {
		max-width: 660px;
		margin: 0 auto;
		color: #aaa9ae;
		font-size: 16px;
		line-height: 1.55;
		text-wrap: balance;
	}
	.hero-actions {
		display: flex;
		justify-content: center;
		gap: 10px;
		margin-top: 27px;
	}
	.pill-light {
		background: #f6f7f4;
		color: #111114;
	}
	.pill-ghost {
		background: #262529;
		color: #f4f4f5;
	}
	.trust-line {
		margin: 22px 0 0;
		color: #747378;
		font-size: 11px;
		letter-spacing: 0.01em;
	}
	.demo-shell {
		max-width: 1120px;
		margin: 68px auto 0;
		overflow: hidden;
		border: 1px solid rgba(255, 255, 255, 0.13);
		border-radius: 17px;
		background: #09090a;
		box-shadow:
			0 42px 100px rgba(0, 0, 0, 0.5),
			0 0 80px rgba(147, 145, 254, 0.09);
	}
	.window-bar {
		height: 42px;
		display: grid;
		grid-template-columns: 1fr min(36%, 340px) 1fr;
		align-items: center;
		padding: 0 14px;
		background: #202023;
	}
	.window-bar::after {
		content: '';
	}
	.address-bar {
		display: flex;
		height: 26px;
		align-items: center;
		justify-content: center;
		border: 1px solid rgba(255, 255, 255, 0.035);
		border-radius: 7px;
		background: #171719;
		color: #8f8e93;
		font-size: 10px;
	}
	.traffic {
		display: flex;
		gap: 6px;
	}
	.traffic i {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: #46454a;
	}
	.demo-shell iframe {
		display: block;
		width: 100%;
		height: 670px;
		border: 0;
		background: #17171a;
	}
	.markets-copy {
		max-width: 1320px;
		margin: 106px auto 102px;
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: 48px;
		align-items: start;
	}
	.markets-copy h2 {
		margin: 0 0 24px;
		font-size: clamp(40px, 3.2vw, 54px);
		line-height: 1.08;
		letter-spacing: -0.04em;
		font-weight: 560;
	}
	.markets-text > p {
		max-width: 980px;
		margin: 0;
		color: #aaa7af;
		font-size: clamp(18px, 1.35vw, 23px);
		line-height: 1.5;
	}
	.markets-cta {
		display: inline-flex;
		min-height: 68px;
		align-items: center;
		justify-content: center;
		margin-top: 1px;
		padding: 0 34px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.1);
		color: #f6f4f8;
		text-decoration: none;
		font-size: 18px;
		font-weight: 650;
		transition:
			transform 180ms ease,
			background 180ms ease;
	}
	.markets-cta:hover {
		transform: translateY(-2px);
		background: rgba(255, 255, 255, 0.14);
	}
	.product-followup h3 {
		margin-bottom: 0;
		font-size: clamp(34px, 4vw, 52px);
		line-height: 1.08;
		letter-spacing: -0.045em;
		font-weight: 540;
	}
	.product-followup > p {
		max-width: 500px;
		margin: 0;
		color: #96959b;
		line-height: 1.6;
	}
	.feature-grid {
		max-width: 1320px;
		margin: 0 auto;
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 32px;
	}
	.feature-card {
		min-height: 476px;
		padding: 34px 32px 30px;
		border-radius: 30px;
		background: rgba(43, 36, 51, 0.72);
		display: flex;
		flex-direction: column;
	}
	.feature-label {
		color: #958cff;
		font-size: 17px;
		font-weight: 650;
	}
	.feature-card h3 {
		margin: 16px 0 18px;
		font-size: 27px;
		line-height: 1.15;
		letter-spacing: -0.035em;
		font-weight: 560;
	}
	.feature-card p {
		margin: 0;
		color: #aaa7af;
		font-size: 19px;
		line-height: 1.5;
	}
	.feature-footer {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin-top: auto;
		padding-top: 45px;
		border-top: 1px solid rgba(255, 255, 255, 0.12);
	}
	.feature-chip {
		display: inline-flex;
		min-height: 30px;
		align-items: center;
		gap: 7px;
		padding: 0 11px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.07);
		color: #c4c1c8;
		font-size: 14px;
		white-space: nowrap;
	}
	.feature-chip i {
		width: 9px;
		height: 9px;
		border: 2px solid #958cff;
		border-radius: 50%;
	}
	.product-world {
		margin: 0 24px;
		border-radius: 32px;
		overflow: hidden;
		padding: 118px 48px 58px;
		background:
			radial-gradient(
				circle at 50% 44%,
				rgba(147, 145, 254, 0.42),
				transparent 32%
			),
			linear-gradient(180deg, #fff 4%, #f2f0ff 48%, #dedcff 100%);
	}
	.section-intro {
		max-width: 840px;
		margin: 0 auto;
		text-align: center;
	}
	.section-intro h2 {
		margin: 22px auto 14px;
		font-size: clamp(45px, 5vw, 68px);
		line-height: 1.07;
		letter-spacing: -0.05em;
		font-weight: 560;
		text-wrap: balance;
	}
	.section-intro p {
		max-width: 610px;
		margin: 0 auto;
		color: #6f6d76;
		text-wrap: balance;
	}
	.pill-lavender {
		background: #9391fe;
		color: #111114;
	}
	.pill-outline {
		border: 1px solid rgba(15, 15, 16, 0.14);
		background: rgba(255, 255, 255, 0.35);
	}
	.product-stage {
		position: relative;
		max-width: 1120px;
		height: 560px;
		margin: 92px auto 0;
		border: 1px solid rgba(255, 255, 255, 0.72);
		border-radius: 20px;
		overflow: hidden;
		background: rgba(15, 15, 16, 0.96);
		box-shadow: 0 50px 100px rgba(83, 73, 145, 0.25);
	}
	.mini-sidebar {
		position: absolute;
		inset: 0 auto 0 0;
		width: 62px;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 22px;
		padding: 22px 0;
		background: #1b1b1e;
		border-right: 1px solid #2b2a2f;
	}
	.mini-brand {
		display: grid;
		width: 28px;
		height: 28px;
		place-items: center;
		border-radius: 8px;
		background: #9391fe;
		color: #111114;
		font-weight: 800;
	}
	.mini-sidebar i {
		width: 20px;
		height: 20px;
		border-radius: 6px;
		background: #333238;
	}
	.mini-sidebar i.active {
		background: #9391fe;
	}
	.mini-panel {
		position: absolute;
		border: 1px solid #37363d;
		border-radius: 13px;
		background: #202024;
		color: #f5f5f3;
		box-shadow: 0 30px 60px rgba(0, 0, 0, 0.25);
	}
	.task-panel {
		left: 105px;
		top: 70px;
		width: 48%;
		padding: 28px;
		transform: rotate(-2deg);
	}
	.planner-panel {
		right: 60px;
		bottom: 62px;
		width: 43%;
		min-height: 305px;
		padding: 28px;
		transform: rotate(2deg);
	}
	.mini-label {
		color: #8c8b91;
		font-size: 10px;
		letter-spacing: 0.14em;
	}
	.task-panel h3 {
		margin: 14px 0 30px;
		font-size: 28px;
		letter-spacing: -0.04em;
	}
	.task-row {
		height: 54px;
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: 12px;
		border-top: 1px solid #333238;
		font-size: 13px;
	}
	.task-row i {
		width: 17px;
		height: 17px;
		border: 1px solid #64626b;
		border-radius: 50%;
	}
	.task-row i.done {
		background: #9391fe;
		border-color: #9391fe;
		box-shadow: inset 0 0 0 4px #202024;
	}
	.task-row small {
		color: #77757e;
	}
	.time-slot {
		display: grid;
		grid-template-columns: 55px 1fr;
		align-items: center;
		height: 70px;
		margin-top: 13px;
		padding: 0 18px;
		border-left: 3px solid #56545e;
		background: #29282e;
		font-size: 13px;
	}
	.time-slot small {
		color: #838188;
	}
	.time-slot.lavender {
		border-left-color: #9391fe;
		background: rgba(147, 145, 254, 0.13);
	}
	.time-slot.short {
		width: 78%;
	}
	.focus-orb {
		position: absolute;
		left: 49%;
		top: 41%;
		z-index: 2;
		display: grid;
		width: 142px;
		height: 142px;
		place-content: center;
		border: 1px solid rgba(229, 229, 255, 0.55);
		border-radius: 50%;
		background: radial-gradient(circle, #9391fe, #5f5dc2 70%, #272633 71%);
		color: #fff;
		text-align: center;
		box-shadow: 0 0 55px rgba(147, 145, 254, 0.45);
	}
	.focus-orb span {
		font-size: 25px;
		font-weight: 680;
		letter-spacing: -0.04em;
	}
	.focus-orb small {
		margin-top: 4px;
		font-size: 8px;
		letter-spacing: 0.18em;
	}
	.product-followup {
		max-width: 1120px;
		margin: 82px auto 0;
		display: grid;
		grid-template-columns: 1.1fr 0.9fr;
		gap: 70px;
		align-items: end;
	}
	.product-followup > p {
		color: #5f5d68;
	}
	.platforms,
	.trust,
	.insights,
	.faq {
		max-width: 1320px;
		margin: 0 auto;
		padding: 132px 48px 0;
	}
	.wide-heading,
	.insights-heading {
		display: grid;
		grid-template-columns: 1.1fr 0.9fr;
		gap: 70px;
		align-items: end;
		margin-bottom: 42px;
	}
	.wide-heading h2,
	.trust-intro h2,
	.insights-heading h2 {
		margin: 0;
		font-size: clamp(42px, 5vw, 66px);
		line-height: 1.05;
		letter-spacing: -0.05em;
		font-weight: 560;
	}
	.wide-heading > p,
	.insights-heading > p {
		max-width: 440px;
		margin: 0;
		color: #706e76;
	}
	.platform-grid {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		border: 1px solid var(--line);
	}
	.platform-card {
		min-height: 215px;
		display: flex;
		flex-direction: column;
		justify-content: space-between;
		padding: 24px;
		border-right: 1px solid var(--line);
		text-decoration: none;
		background: #fff;
	}
	.platform-card:last-child {
		border-right: 0;
	}
	.platform-icon {
		display: grid;
		width: 42px;
		height: 42px;
		place-items: center;
		border-radius: 12px;
		background: var(--mist);
		color: #6663d8;
		font-weight: 800;
	}
	.platform-card h3 {
		margin: 0 0 5px;
		font-size: 20px;
		letter-spacing: -0.03em;
	}
	.platform-card p {
		margin: 0;
		color: #8a8890;
		font-size: 12px;
	}
	.soon {
		align-self: flex-start;
		border-radius: 999px;
		padding: 5px 9px;
		background: #f3f2f6;
		color: #8d8b92;
		font-size: 10px;
	}
	.web-card {
		background: #0f0f10;
		color: #f6f7f4;
		transition: background 180ms ease;
	}
	.web-card:hover {
		background: #1d1927;
	}
	.web-card .platform-icon {
		background: #9391fe;
		color: #0f0f10;
	}
	.arrow {
		align-self: flex-end;
		font-size: 24px;
		color: #9391fe;
	}
	.trust {
		padding-top: 150px;
	}
	.trust-intro {
		max-width: 700px;
	}
	.trust-intro p {
		max-width: 630px;
		margin: 24px 0 0;
		color: #706e76;
		font-size: 17px;
	}
	.proof-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		margin-top: 72px;
		border-top: 1px solid var(--line);
		border-left: 1px solid var(--line);
	}
	.proof-grid article {
		min-height: 190px;
		padding: 26px;
		border-right: 1px solid var(--line);
		border-bottom: 1px solid var(--line);
	}
	.proof-grid span {
		color: #9391fe;
		font-size: 10px;
		font-weight: 780;
	}
	.proof-grid h3 {
		margin: 34px 0 6px;
		font-size: 24px;
		letter-spacing: -0.04em;
	}
	.proof-grid p {
		margin: 0;
		color: #85838b;
		font-size: 12px;
	}
	.insights {
		padding-top: 158px;
	}
	.chart-card {
		padding: 34px 40px 28px;
		border: 1px solid var(--line);
		border-radius: 2px;
		background: linear-gradient(180deg, #fff, #fbfaff);
		box-shadow: 0 24px 70px rgba(68, 60, 111, 0.06);
	}
	.chart-legend {
		display: flex;
		justify-content: flex-end;
		gap: 18px;
		color: #77757d;
		font-size: 10px;
	}
	.chart-legend span {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.chart-legend i {
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}
	.chart-legend .planned {
		background: #d9d8df;
	}
	.chart-legend .focused {
		background: #9391fe;
	}
	.chart-plot {
		height: 360px;
		display: grid;
		grid-template-columns: repeat(7, 1fr);
		gap: 20px;
		align-items: stretch;
		margin-top: 25px;
		padding: 20px 25px 0;
		background: repeating-linear-gradient(
			to top,
			transparent 0,
			transparent 71px,
			#eeedf2 72px
		);
	}
	.day {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.bars {
		flex: 1;
		display: flex;
		gap: 7px;
		align-items: end;
		justify-content: center;
	}
	.bars i {
		width: min(28%, 28px);
		border-radius: 5px 5px 0 0;
	}
	.planned-bar {
		height: var(--planned);
		background: #d9d8df;
	}
	.focus-bar {
		height: var(--focused);
		background: linear-gradient(180deg, #bab8ff, #7673e9);
	}
	.day > span {
		height: 28px;
		padding-top: 8px;
		text-align: center;
		color: #89878f;
		font-size: 10px;
	}
	.faq {
		display: grid;
		grid-template-columns: 0.7fr 1.3fr;
		gap: 80px;
		padding-top: 170px;
		padding-bottom: 150px;
	}
	.faq h2 {
		font-size: 40px;
		letter-spacing: -0.05em;
		font-weight: 560;
	}
	.faq-list {
		border-top: 1px solid var(--line);
	}
	details {
		border-bottom: 1px solid var(--line);
		padding: 0 4px;
	}
	summary {
		min-height: 65px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 20px;
		cursor: pointer;
		list-style: none;
		font-weight: 650;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary span {
		color: #9391fe;
		font-size: 20px;
		font-weight: 400;
		transition: transform 180ms ease;
	}
	details[open] summary span {
		transform: rotate(45deg);
	}
	details p {
		max-width: 650px;
		margin: -2px 38px 22px 0;
		color: #77757d;
		font-size: 13px;
		line-height: 1.65;
	}
	footer {
		margin: 0 24px 24px;
		padding: 48px;
		border-radius: 28px;
		background: #f6f5f8;
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 70px;
	}
	.footer-brand span {
		font-size: 20px;
		font-weight: 780;
		letter-spacing: -0.04em;
	}
	.footer-brand p {
		margin: 10px 0 0;
		color: #77757d;
		font-size: 12px;
	}
	footer nav {
		display: flex;
		gap: 28px;
		align-items: start;
	}
	footer nav a {
		text-decoration: none;
		font-size: 11px;
		font-weight: 650;
	}
	.copyright {
		grid-column: 1/-1;
		margin: 28px 0 0;
		padding-top: 24px;
		border-top: 1px solid #e0dfe4;
		color: #96949b;
		font-size: 10px;
	}
	@media (max-width: 900px) {
		.dark-world,
		.product-world {
			margin-left: 10px;
			margin-right: 10px;
			padding-left: 24px;
			padding-right: 24px;
			border-radius: 24px;
		}
		.dark-world {
			padding-top: 64px;
		}
		.site-header {
			padding: 0 18px;
		}
		.header-download {
			display: none;
		}
		.demo-shell iframe {
			height: 560px;
		}
		.markets-copy,
		.product-followup,
		.wide-heading,
		.insights-heading {
			grid-template-columns: 1fr;
			gap: 18px;
		}
		.markets-cta {
			justify-self: start;
		}
		.feature-grid,
		.proof-grid {
			grid-template-columns: 1fr 1fr;
		}
		.platform-grid {
			grid-template-columns: 1fr 1fr;
		}
		.platform-card:nth-child(2) {
			border-right: 0;
		}
		.platform-card:nth-child(-n + 2) {
			border-bottom: 1px solid var(--line);
		}
		.product-stage {
			height: 510px;
		}
		.focus-orb {
			left: 43%;
		}
		.faq {
			grid-template-columns: 1fr;
			gap: 20px;
		}
		footer {
			grid-template-columns: 1fr;
		}
		footer nav {
			flex-wrap: wrap;
		}
	}
	@media (max-width: 600px) {
		.site-header {
			height: 68px;
		}
		.brand span {
			display: none;
		}
		.pill {
			min-height: 36px;
			padding: 0 15px;
			font-size: 12px;
		}
		.dark-world {
			padding: 52px 16px 28px;
		}
		h1 {
			font-size: 43px;
		}
		.hero-copy {
			font-size: 14px;
		}
		.hero-actions {
			flex-wrap: wrap;
		}
		.trust-line {
			line-height: 1.7;
		}
		.demo-shell {
			margin-top: 58px;
			border-radius: 12px;
		}
		.demo-shell iframe {
			height: 590px;
		}
		.markets-copy {
			margin: 76px auto 48px;
		}
		.feature-grid,
		.proof-grid,
		.platform-grid {
			grid-template-columns: 1fr;
		}
		.feature-card {
			min-height: 360px;
			padding: 28px 24px 24px;
		}
		.product-world {
			padding: 84px 16px 32px;
		}
		.section-intro h2 {
			font-size: 44px;
		}
		.product-stage {
			height: 520px;
			margin-top: 60px;
		}
		.mini-sidebar {
			width: 45px;
		}
		.task-panel {
			left: 65px;
			top: 45px;
			width: 80%;
		}
		.planner-panel {
			right: 24px;
			bottom: 38px;
			width: 76%;
		}
		.focus-orb {
			left: 38%;
			top: 39%;
			width: 112px;
			height: 112px;
		}
		.product-followup {
			margin-top: 60px;
		}
		.platforms,
		.trust,
		.insights,
		.faq {
			padding-left: 20px;
			padding-right: 20px;
			padding-top: 100px;
		}
		.platform-card {
			border-right: 0;
			border-bottom: 1px solid var(--line);
		}
		.proof-grid {
			border-left: 0;
		}
		.proof-grid article {
			border-left: 1px solid var(--line);
		}
		.chart-card {
			padding: 22px 12px 16px;
		}
		.chart-plot {
			height: 290px;
			gap: 8px;
			padding-left: 5px;
			padding-right: 5px;
		}
		.faq {
			padding-bottom: 110px;
		}
		footer {
			margin: 0 10px 10px;
			padding: 30px 24px;
		}
		footer nav {
			display: grid;
			grid-template-columns: 1fr 1fr;
			gap: 16px;
		}
	}
</style>
