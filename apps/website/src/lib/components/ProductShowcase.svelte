<script lang="ts">
	import { asset } from '$app/paths';
	import { onMount } from 'svelte';
	import Icon, { type IconName } from './Icon.svelte';

	type Screen = {
		id: string;
		label: string;
		icon: IconName;
		src: string;
		alt: string;
		caption: string;
	};

	const screens: Screen[] = [
		{
			id: 'files',
			label: 'Files',
			icon: 'note',
			src: asset('/screens/files.webp'),
			alt: 'noura with the Website relaunch note open in the Markdown editor, next to the sidebar’s folders and notes',
			caption: 'Each note is a Markdown file in a folder you choose.',
		},
		{
			id: 'tasks',
			label: 'Tasks',
			icon: 'checks',
			src: asset('/screens/tasks.webp'),
			alt: 'The Spring launch project in noura’s Tasks view, with tasks grouped by due date and one task, assigned to Theo, open beside the list',
			caption:
				'Track due dates and priorities. Each task is its own Markdown file.',
		},
		{
			id: 'calendar',
			label: 'Calendar',
			icon: 'calendar',
			src: asset('/screens/calendar.webp'),
			alt: 'The noura calendar showing November 2026 by month, with events from four calendars and tasks on their due dates',
			caption: 'See events and due tasks by month, week, or day.',
		},
		{
			id: 'ai',
			label: 'AI',
			icon: 'sparkle',
			src: asset('/screens/ai.webp'),
			alt: 'An AI chat in noura answering what’s left before the website relaunch, then asking “Allow this tool call?” before it creates a task',
			caption:
				'Chat with your own AI provider. noura asks before each tool the AI runs.',
		},
	];

	/** How long each screen shows before the next one, in milliseconds. Matches the fill animation. */
	const advanceAfter = 6000;

	const uid = $props.id();
	const tabId = (index: number) => `${uid}-tab-${screens[index].id}`;
	const panelId = (index: number) => `${uid}-panel-${screens[index].id}`;

	let root: HTMLElement;
	let active = $state(0);
	/** Turns off for good once the visitor picks a tab. */
	let autoplay = $state(true);
	let mounted = $state(false);
	let reducedMotion = $state(false);
	let hovered = $state(false);
	let focused = $state(false);
	let inView = $state(false);
	let pageHidden = $state(false);
	/** Once true, every screenshot gets its `src`, so the browser fetches the rest. */
	let warm = $state(false);

	const cycling = $derived(mounted && autoplay && !reducedMotion);
	const paused = $derived(hovered || focused || !inView || pageHidden);

	function choose(index: number) {
		autoplay = false;
		warm = true;
		active = index;
	}

	function advance() {
		if (cycling) active = (active + 1) % screens.length;
	}

	function onTabKeydown(event: KeyboardEvent) {
		const last = screens.length - 1;
		const next =
			event.key === 'ArrowRight'
				? active === last
					? 0
					: active + 1
				: event.key === 'ArrowLeft'
					? active === 0
						? last
						: active - 1
					: event.key === 'Home'
						? 0
						: event.key === 'End'
							? last
							: null;
		if (next === null) return;
		event.preventDefault();
		choose(next);
		document.getElementById(tabId(next))?.focus();
	}

	function onFocusOut(event: FocusEvent) {
		const target = event.relatedTarget;
		focused = target instanceof Node && root.contains(target);
	}

	onMount(() => {
		mounted = true;

		const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
		reducedMotion = motion.matches;
		const onMotion = (event: MediaQueryListEvent) =>
			(reducedMotion = event.matches);
		motion.addEventListener('change', onMotion);

		const onEnter = () => (hovered = true);
		const onLeave = () => (hovered = false);
		root.addEventListener('pointerenter', onEnter);
		root.addEventListener('pointerleave', onLeave);

		pageHidden = document.hidden;
		const onVisibility = () => (pageHidden = document.hidden);
		document.addEventListener('visibilitychange', onVisibility);

		const observer = new IntersectionObserver(
			([entry]) => (inView = entry.isIntersecting),
			{ threshold: 0.25 },
		);
		observer.observe(root);

		const warmUp = () => (warm = true);
		let cancelIdle: () => void;
		if (typeof window.requestIdleCallback === 'function') {
			const handle = window.requestIdleCallback(warmUp, { timeout: 3000 });
			cancelIdle = () => window.cancelIdleCallback(handle);
		} else {
			const handle = setTimeout(warmUp, 1500);
			cancelIdle = () => clearTimeout(handle);
		}

		return () => {
			motion.removeEventListener('change', onMotion);
			root.removeEventListener('pointerenter', onEnter);
			root.removeEventListener('pointerleave', onLeave);
			document.removeEventListener('visibilitychange', onVisibility);
			observer.disconnect();
			cancelIdle();
		};
	});
</script>

<div
	class="showcase"
	bind:this={root}
	onfocusin={() => (focused = true)}
	onfocusout={onFocusOut}
>
	<div class="tabs-row">
		<div class="tabs" role="tablist" aria-label="Product tour">
			{#each screens as screen, index (screen.id)}
				{@const selected = index === active}
				<button
					type="button"
					role="tab"
					class="tab"
					id={tabId(index)}
					aria-selected={selected}
					aria-controls={panelId(index)}
					tabindex={selected ? 0 : -1}
					onclick={() => choose(index)}
					onkeydown={onTabKeydown}
				>
					{#if selected && cycling}
						<span
							class="fill"
							class:paused
							style:animation-duration={`${advanceAfter}ms`}
							onanimationend={advance}
							aria-hidden="true"
						></span>
					{/if}
					<Icon name={screen.icon} size={17} />
					<span>{screen.label}</span>
				</button>
			{/each}
		</div>
	</div>

	<div class="stage">
		{#each screens as screen, index (screen.id)}
			<div
				class="panel"
				class:active={index === active}
				role="tabpanel"
				id={panelId(index)}
				aria-labelledby={tabId(index)}
				tabindex={index === active ? 0 : -1}
			>
				<div class="window">
					<img
						src={index === 0 || warm ? screen.src : undefined}
						alt={screen.alt}
						width="2400"
						height="1560"
						decoding="async"
						loading={index === 0 ? 'eager' : undefined}
						fetchpriority={index === 0 ? 'high' : undefined}
					/>
				</div>
				<p class="caption">{screen.caption}</p>
			</div>
		{/each}
	</div>
</div>

<style>
	.showcase {
		container-type: inline-size;
	}

	/* Tabs */
	.tabs-row {
		display: flex;
		justify-content: center;
		margin-bottom: 32px;
	}
	.tabs {
		display: flex;
		gap: 4px;
		max-width: 100%;
		padding: 5px;
		overflow-x: auto;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.09);
		scrollbar-width: none;
	}
	.tabs::-webkit-scrollbar {
		display: none;
	}
	.tab {
		position: relative;
		isolation: isolate;
		display: inline-flex;
		flex: none;
		min-height: 40px;
		align-items: center;
		gap: 8px;
		padding: 0 18px;
		overflow: hidden;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: #a9a7b0;
		font-size: 14px;
		font-weight: 650;
		white-space: nowrap;
		cursor: pointer;
		transition:
			color 180ms ease,
			background 180ms ease;
	}
	.tab:hover {
		color: #ecebef;
	}
	.tab:focus-visible {
		outline-offset: -2px;
	}
	.tab[aria-selected='true'] {
		background: rgba(255, 255, 255, 0.1);
		color: #f6f7f4;
	}
	.tab[aria-selected='true'] :global(svg) {
		color: #c4c2ff;
	}
	.fill {
		position: absolute;
		inset: 0;
		z-index: -1;
		background: rgba(147, 145, 254, 0.24);
		transform: scaleX(0);
		transform-origin: left;
		animation-name: fill;
		animation-timing-function: linear;
		animation-fill-mode: forwards;
	}
	.fill.paused {
		animation-play-state: paused;
	}
	@keyframes fill {
		to {
			transform: scaleX(1);
		}
	}

	/* Screens */
	.stage {
		display: grid;
	}
	.panel {
		grid-area: 1 / 1;
		min-width: 0;
		border-radius: 12px;
		opacity: 0;
		visibility: hidden;
		/* The outgoing screen stays put under the incoming one, then drops out. */
		transition:
			opacity 0s linear 250ms,
			visibility 0s linear 250ms;
	}
	.panel.active {
		z-index: 1;
		opacity: 1;
		visibility: visible;
		transition: opacity 250ms ease;
	}
	.panel:focus-visible {
		outline-offset: 8px;
	}
	.window {
		overflow: hidden;
		border-radius: 12px;
		border-radius: min(12px, 1cqi);
		background: #f4f4f5;
		box-shadow:
			0 0 0 1px rgba(255, 255, 255, 0.14),
			0 32px 80px -24px rgba(0, 0, 0, 0.75),
			0 60px 140px -40px rgba(147, 145, 254, 0.32);
	}
	.window img {
		display: block;
		width: 100%;
		height: auto;
		aspect-ratio: 2400 / 1560;
	}
	.caption {
		max-width: 560px;
		margin: 28px auto 0;
		color: #b3b1b8;
		font-size: 16px;
		line-height: 1.5;
		text-align: center;
		text-wrap: balance;
		transition: opacity 120ms ease;
	}
	.panel:not(.active) .caption {
		opacity: 0;
	}

	@media (prefers-reduced-motion: reduce) {
		.panel,
		.panel.active,
		.caption {
			transition: none;
		}
	}
	@media (max-width: 600px) {
		.tabs-row {
			margin-bottom: 24px;
		}
		.tabs {
			gap: 2px;
			padding: 4px;
		}
		.tab {
			min-height: 38px;
			gap: 6px;
			padding: 0 11px;
			font-size: 13.5px;
		}
		.caption {
			margin-top: 20px;
			font-size: 15px;
		}
	}
</style>
