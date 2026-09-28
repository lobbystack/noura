<script lang="ts">
	import Icon, { type IconName } from './Icon.svelte';

	const rail: ReadonlyArray<{ icon: IconName; label: string }> = [
		{ icon: 'inbox', label: 'Inbox' },
		{ icon: 'sparkle', label: 'AI' },
		{ icon: 'note', label: 'Notes' },
		{ icon: 'checks', label: 'Tasks' },
		{ icon: 'calendar', label: 'Calendar' },
		{ icon: 'folder', label: 'Projects' },
	];

	const notes = [
		'Launch brief',
		'Customer interviews',
		'Pricing research',
		'Weekly review',
	] as const;

	const tasks = [
		{ title: 'Draft the announcement post', done: true, due: 'Sep 30' },
		{ title: 'Prepare release notes', done: false, due: 'Oct 4' },
		{ title: 'Record the product walkthrough', done: false, due: 'Oct 8' },
	] as const;
</script>

<div
	class="mockup"
	role="img"
	aria-label="The noura app showing a note with linked tasks, next to the Markdown file noura saves for one of those tasks"
>
	<div class="window">
		<div class="titlebar">
			<div class="traffic"><i></i><i></i><i></i></div>
			<span>Studio</span>
		</div>
		<div class="app">
			<nav class="rail">
				{#each rail as item (item.label)}
					<span class:active={item.label === 'Notes'} title={item.label}>
						<Icon name={item.icon} size={17} />
					</span>
				{/each}
				<span class="rail-bottom"><Icon name="search" size={17} /></span>
				<span><Icon name="gear" size={17} /></span>
			</nav>

			<aside class="sidebar">
				<div class="workspace">Studio <small>▾</small></div>
				<p class="group">Notes</p>
				{#each notes as note (note)}
					<div class="item" class:current={note === 'Launch brief'}>
						<Icon name="file" size={14} />{note}
					</div>
				{/each}
				<p class="group">Projects</p>
				<div class="item"><i class="dot"></i>Website relaunch</div>
				<div class="item"><i class="dot alt"></i>Q4 roadmap</div>
			</aside>

			<article class="editor">
				<div class="crumbs">Notes / Launch brief</div>
				<h4>Launch brief</h4>
				<p>
					Ship the public alpha before the conference. Keep the message simple:
					your notes and tasks stay in files you own.
				</p>
				<p class="subhead">Next steps</p>
				<ul>
					{#each tasks as task (task.title)}
						<li class:highlight={task.title === 'Prepare release notes'}>
							<i class="box" class:done={task.done}></i>
							<span class:struck={task.done}>{task.title}</span>
							<small>{task.due}</small>
						</li>
					{/each}
				</ul>
				<div class="tags"><span>#launch</span><span>#website</span></div>
			</article>
		</div>
	</div>

	<div class="file-card">
		<div class="file-head">
			<Icon name="file" size={14} />
			<span>tasks/prepare-release-notes.md</span>
		</div>
		<pre><span class="dim">---</span>
<span class="key">id:</span> task_01k8q3v2x7m4
<span class="key">type:</span> task
<span class="key">status:</span> todo
<span class="key">priority:</span> medium
<span class="key">due:</span> 2026-10-04
<span class="dim">---</span>

<span class="heading"># Prepare release notes</span>

Summarize what changed for users.</pre>
		<p class="file-foot">Plain Markdown. Opens in any editor.</p>
	</div>
</div>

<style>
	.mockup {
		position: relative;
		max-width: 1120px;
		margin: 0 auto;
		padding-right: 0;
	}
	.window {
		overflow: hidden;
		border: 1px solid rgba(255, 255, 255, 0.12);
		border-radius: 16px;
		background: #141416;
		box-shadow:
			0 42px 100px rgba(0, 0, 0, 0.5),
			0 0 80px rgba(147, 145, 254, 0.1);
		color: #e9e8ec;
		text-align: left;
	}
	.titlebar {
		position: relative;
		height: 40px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: #1c1c1f;
		border-bottom: 1px solid #28272c;
		color: #8f8e94;
		font-size: 12px;
		font-weight: 600;
	}
	.traffic {
		position: absolute;
		left: 16px;
		display: flex;
		gap: 7px;
	}
	.traffic i {
		width: 11px;
		height: 11px;
		border-radius: 50%;
		background: #3d3c42;
	}
	.app {
		display: grid;
		grid-template-columns: 56px 232px 1fr;
		height: 540px;
	}
	.rail {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		padding: 14px 0;
		background: #18181b;
		border-right: 1px solid #26252a;
		color: #86858c;
	}
	.rail span {
		display: grid;
		width: 36px;
		height: 36px;
		place-items: center;
		border-radius: 9px;
	}
	.rail span.active {
		background: rgba(147, 145, 254, 0.16);
		color: #b9b8ff;
	}
	.rail-bottom {
		margin-top: auto;
	}
	.sidebar {
		padding: 16px 12px;
		background: #18181b;
		border-right: 1px solid #26252a;
		font-size: 13px;
	}
	.workspace {
		padding: 4px 8px 14px;
		font-weight: 700;
	}
	.workspace small {
		color: #77767c;
	}
	.group {
		margin: 12px 8px 6px;
		color: #6f6e75;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}
	.item {
		display: flex;
		align-items: center;
		gap: 9px;
		height: 32px;
		padding: 0 8px;
		border-radius: 7px;
		color: #b3b2b8;
	}
	.item.current {
		background: #26252b;
		color: #f3f2f6;
	}
	.dot {
		width: 8px;
		height: 8px;
		margin: 0 3px;
		border-radius: 3px;
		background: #9391fe;
	}
	.dot.alt {
		background: #e7a86b;
	}
	.editor {
		padding: 30px 56px;
		background: #141416;
		overflow: hidden;
	}
	.crumbs {
		color: #6f6e75;
		font-size: 12px;
	}
	.editor h4 {
		margin: 20px 0 14px;
		font-size: 30px;
		font-weight: 650;
		letter-spacing: -0.03em;
	}
	.editor p {
		max-width: 520px;
		margin: 0;
		color: #b3b2b8;
		font-size: 14.5px;
		line-height: 1.65;
	}
	.editor .subhead {
		margin-top: 26px;
		color: #f3f2f6;
		font-size: 15px;
		font-weight: 700;
	}
	ul {
		max-width: 440px;
		margin: 10px 0 0;
		padding: 0;
		list-style: none;
	}
	li {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: 12px;
		height: 42px;
		padding: 0 12px;
		margin: 0 -12px;
		border-radius: 8px;
		font-size: 14px;
	}
	li.highlight {
		background: rgba(147, 145, 254, 0.1);
		box-shadow: inset 0 0 0 1px rgba(147, 145, 254, 0.35);
	}
	li small {
		color: #77767c;
		font-size: 12px;
	}
	.box {
		width: 16px;
		height: 16px;
		border: 1.5px solid #5c5b62;
		border-radius: 5px;
	}
	.box.done {
		border-color: #9391fe;
		background: #9391fe;
		box-shadow: inset 0 0 0 3px #141416;
	}
	.struck {
		color: #77767c;
		text-decoration: line-through;
	}
	.tags {
		display: flex;
		gap: 8px;
		margin-top: 22px;
	}
	.tags span {
		padding: 3px 9px;
		border-radius: 6px;
		background: #222126;
		color: #a3a2a9;
		font-size: 12px;
	}
	.file-card {
		position: absolute;
		right: -28px;
		bottom: -44px;
		width: 310px;
		overflow: hidden;
		border: 1px solid rgba(255, 255, 255, 0.7);
		border-radius: 14px;
		background: #f7f6fb;
		color: #1a1a1d;
		text-align: left;
		box-shadow: 0 30px 70px rgba(0, 0, 0, 0.45);
	}
	.file-head {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 11px 16px;
		border-bottom: 1px solid #e3e1ea;
		color: #55545c;
		font-size: 12px;
		font-weight: 650;
	}
	pre {
		margin: 0;
		padding: 14px 16px 6px;
		font-family: ui-monospace, 'SF Mono', Menlo, monospace;
		font-size: 12px;
		line-height: 1.65;
		white-space: pre-wrap;
	}
	.dim {
		color: #a3a1ab;
	}
	.key {
		color: #5d5ad6;
	}
	.heading {
		font-weight: 700;
	}
	.file-foot {
		margin: 0;
		padding: 10px 16px 14px;
		color: #75737d;
		font-size: 12px;
	}
	@media (max-width: 1100px) {
		.file-card {
			right: 16px;
		}
	}
	@media (max-width: 900px) {
		.app {
			grid-template-columns: 52px 1fr;
			height: 500px;
		}
		.sidebar {
			display: none;
		}
		.editor {
			padding: 26px 28px;
		}
	}
	@media (max-width: 600px) {
		.app {
			grid-template-columns: 1fr;
			height: auto;
		}
		.rail {
			display: none;
		}
		.editor {
			padding: 22px 20px 26px;
		}
		.editor h4 {
			font-size: 24px;
		}
		li small {
			display: none;
		}
		.file-card {
			position: relative;
			right: auto;
			bottom: auto;
			width: auto;
			margin: -18px 14px 0;
		}
	}
</style>
