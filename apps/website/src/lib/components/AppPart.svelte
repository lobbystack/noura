<script lang="ts" module>
	/** Viewports this narrow get a part's `compact` crop. Keep in step with the media query in the styles. */
	const compactMedia = '(max-width: 600px)';

	type Part = {
		/** Image size in pixels; the page draws it at half that in CSS pixels. */
		width: number;
		height: number;
		alt: string;
		/**
		 * False when the image carries its own rounded corners and shadow on a
		 * transparent background, so the page adds no frame around it.
		 */
		framed?: boolean;
		/** A narrower crop of the same interface for phones. */
		compact?: { file: string; width: number; height: number };
	};

	/**
	 * Real pieces of the app's interface, cropped at 2 pixels per point by
	 * `scripts/website-screens.sh`. See `static/parts/README.md`.
	 */
	export const parts = {
		'ai-dialog': {
			width: 1044,
			height: 694,
			framed: false,
			alt: 'noura asking “Allow this tool call?” before the AI creates a task, with the task’s title, due date and project, and Deny and Allow once buttons',
		},
		plugins: {
			width: 1416,
			height: 1048,
			alt: 'Plugin settings in noura: AI, Calendar, Projects and Tasks turned on, Dictation and Transcription turned off',
			compact: { file: 'plugins-compact', width: 680, height: 904 },
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
			alt: 'Search for “journal”, listing four matching files, then a task and a note that mention it, such as the line “Move the journal off the old CMS without breaking a single link”',
		},
		'calendar-day': {
			width: 1142,
			height: 866,
			alt: 'Monday in the calendar’s day view, with a task due that day and three events',
		},
	} satisfies Record<string, Part>;

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

	const part: Part = $derived(parts[name]);
	const src = (file: string) => asset(`/parts/${file}-${theme}.webp`);
</script>

<!-- Drawn at 1 CSS pixel per point, never larger, so the 2x crop stays sharp. -->
<picture>
	{#if part.compact}
		<source
			media={compactMedia}
			srcset={src(part.compact.file)}
			width={part.compact.width / 2}
			height={part.compact.height / 2}
		/>
	{/if}
	<img
		class={[
			'part',
			theme,
			{
				fluid,
				framed: part.framed !== false,
				'has-compact': part.compact,
			},
		]}
		src={src(name)}
		alt={part.alt}
		width={part.width / 2}
		height={part.height / 2}
		style:--wide-width={`${part.width / 2}px`}
		style:--compact-width={part.compact
			? `${part.compact.width / 2}px`
			: undefined}
		loading="lazy"
		decoding="async"
	/>
</picture>

<style>
	picture {
		display: contents;
	}
	.part {
		--native-width: var(--wide-width);
		display: block;
		width: calc(var(--native-width) * var(--part-scale, 1));
		max-width: none;
		height: auto;
	}
	.fluid {
		width: 100%;
		max-width: var(--native-width);
	}
	.framed {
		border-radius: 12px;
	}
	.framed.dark {
		box-shadow:
			0 0 0 1px rgba(255, 255, 255, 0.1),
			0 24px 60px -16px rgba(0, 0, 0, 0.7);
	}
	.framed.light {
		box-shadow:
			0 0 0 1px rgba(15, 15, 16, 0.08),
			0 30px 70px -24px rgba(83, 73, 145, 0.4);
	}
	/* Same breakpoint as `compactMedia`. */
	@media (max-width: 600px) {
		.has-compact {
			--native-width: var(--compact-width);
		}
	}
</style>
