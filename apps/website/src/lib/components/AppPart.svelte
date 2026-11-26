<script lang="ts" module>
	/**
	 * Real pieces of the app's interface, cropped at 2 pixels per point by
	 * `scripts/website-screens.sh`. Sizes are in image pixels; see
	 * `static/parts/README.md`.
	 */
	export const parts = {
		'ai-approval': {
			width: 1564,
			height: 1000,
			alt: 'noura asking “Allow this tool call?” before the AI creates a task from a note, with the task’s details and Deny and Allow once buttons',
		},
		plugins: {
			width: 1416,
			height: 1048,
			alt: 'Plugin settings in noura: AI, Calendar, Projects and Tasks turned on, Dictation and Transcription turned off',
		},
		'task-rows': {
			width: 878,
			height: 450,
			alt: 'Three upcoming tasks with their due dates and one completed task',
		},
		'file-tree': {
			width: 444,
			height: 948,
			alt: 'The sidebar’s list of folders and notes, with the Website relaunch note selected',
		},
		mcp: {
			width: 1416,
			height: 794,
			alt: 'The MCP configuration in noura’s settings, with a Copy button and a warning to connect only assistants you trust',
		},
		palette: {
			width: 1208,
			height: 740,
			alt: 'Search for “journal”, matching file names and the text inside notes and tasks',
		},
		'calendar-day': {
			width: 1142,
			height: 866,
			alt: 'Monday in the calendar’s day view, with a task due that day and three events',
		},
	} as const;

	export type PartName = keyof typeof parts;
</script>

<script lang="ts">
	import { asset } from '$app/paths';

	let {
		name,
		theme,
		fluid = false,
	}: {
		name: PartName;
		theme: 'light' | 'dark';
		/** Shrinks with its container instead of overflowing it. */
		fluid?: boolean;
	} = $props();

	const part = $derived(parts[name]);
</script>

<!-- Drawn at 1 CSS pixel per point, never larger, so the 2x crop stays sharp. -->
<img
	class={['part', theme, { fluid }]}
	src={asset(`/parts/${name}-${theme}.webp`)}
	alt={part.alt}
	width={part.width / 2}
	height={part.height / 2}
	style:--part-width={`${part.width / 2}px`}
	loading="lazy"
	decoding="async"
/>

<style>
	.part {
		display: block;
		width: calc(var(--part-width) * var(--part-scale, 1));
		max-width: none;
		height: auto;
		border-radius: 12px;
	}
	.fluid {
		width: 100%;
		max-width: var(--part-width);
	}
	.dark {
		box-shadow:
			0 0 0 1px rgba(255, 255, 255, 0.1),
			0 24px 60px -16px rgba(0, 0, 0, 0.7);
	}
	.light {
		box-shadow:
			0 0 0 1px rgba(15, 15, 16, 0.08),
			0 30px 70px -24px rgba(83, 73, 145, 0.4);
	}
</style>
