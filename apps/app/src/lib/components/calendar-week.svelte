<script lang="ts">
	import type { CalendarTimedSegment, CalendarWeekDay } from '$lib/calendar';
	import { isoDay } from '$lib/calendar';
	import type { CalendarEntry } from '@noura/workspace';
	import { Button } from '$lib/components/ui/button/index.js';
	import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';

	const HOUR_HEIGHT = 56;
	const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

	let {
		days,
		today,
		onopen,
	}: {
		days: CalendarWeekDay[];
		today: Date;
		onopen: (entry: CalendarEntry) => void;
	} = $props();

	const timeFormatter = new Intl.DateTimeFormat(undefined, {
		hour: 'numeric',
		minute: '2-digit',
	});

	function hourLabel(hour: number): string {
		if (hour === 0) return '12 AM';
		if (hour === 12) return '12 PM';
		return `${hour % 12} ${hour < 12 ? 'AM' : 'PM'}`;
	}

	function timeRange(segment: CalendarTimedSegment): string {
		return `${timeFormatter.format(segment.start)}–${timeFormatter.format(segment.end)}`;
	}

	function segmentStyle(segment: CalendarTimedSegment) {
		const width = 100 / segment.laneCount;
		const top = (segment.startMinutes / 60) * HOUR_HEIGHT;
		const height = Math.max(
			((segment.endMinutes - segment.startMinutes) / 60) * HOUR_HEIGHT,
			24,
		);
		return `top:${top}px;height:${height}px;left:calc(${segment.lane * width}% + 0.25rem);width:calc(${width}% - 0.5rem)`;
	}
</script>

<ScrollArea class="min-h-0 flex-1 border-t border-border/60" orientation="both">
	<div class="min-w-[56rem]">
		<div
			class="sticky top-0 z-10 grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))] border-b border-border/60 bg-muted"
		>
			<div aria-hidden="true"></div>
			{#each days as day (isoDay(day.date))}
				<div
					class="flex items-center justify-center gap-1.5 border-l border-border/60 px-2 py-2 text-xs text-muted-foreground"
				>
					<span
						>{day.date.toLocaleDateString(undefined, {
							weekday: 'short',
						})}</span
					>
					<span
						class={{
							'flex size-6 items-center justify-center rounded-full': true,
							'bg-primary font-semibold text-primary-foreground':
								isoDay(day.date) === isoDay(today),
						}}
					>
						{day.date.getDate()}
					</span>
				</div>
			{/each}
		</div>

		<div
			class="sticky top-10 z-10 grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))] border-b border-border/60 bg-background"
		>
			<div class="px-2 py-2 text-right text-xs text-muted-foreground">
				all-day
			</div>
			{#each days as day (isoDay(day.date))}
				<div class="flex min-h-12 flex-col gap-1 border-l border-border/60 p-1">
					{#each day.allDayEntries as entry (entry.sourceId + entry.property)}
						<Button
							variant="secondary"
							size="xs"
							class="w-full justify-start overflow-hidden"
							title={entry.title}
							onclick={() => onopen(entry)}
						>
							<span class="truncate">{entry.title}</span>
						</Button>
					{/each}
				</div>
			{/each}
		</div>

		<div class="grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))]">
			<div class="relative" style:height={`${24 * HOUR_HEIGHT}px`}>
				{#each HOURS as hour (hour)}
					<span
						class="absolute right-2 text-xs tabular-nums text-muted-foreground"
						style:top={`${hour * HOUR_HEIGHT + 4}px`}
					>
						{hourLabel(hour)}
					</span>
				{/each}
			</div>
			{#each days as day (isoDay(day.date))}
				<div
					class="relative border-l border-border/60"
					style:height={`${24 * HOUR_HEIGHT}px`}
				>
					{#each HOURS as hour (hour)}
						<div
							class="pointer-events-none absolute inset-x-0 border-t border-border/50"
							style:top={`${hour * HOUR_HEIGHT}px`}
						></div>
					{/each}
					{#each day.timedSegments as segment (segment.entry.sourceId + segment.entry.property)}
						<Button
							variant="secondary"
							size="xs"
							class="absolute h-auto min-h-6 w-auto flex-col items-start justify-start gap-0 overflow-hidden rounded-sm text-left"
							style={segmentStyle(segment)}
							title={`${segment.entry.title}, ${timeRange(segment)}`}
							onclick={() => onopen(segment.entry)}
						>
							<span class="w-full truncate text-xs font-medium">
								{segment.entry.title}
							</span>
							<span class="w-full truncate text-xs font-normal opacity-70">
								{timeRange(segment)}
							</span>
						</Button>
					{/each}
				</div>
			{/each}
		</div>
	</div>
</ScrollArea>
