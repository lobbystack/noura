<script lang="ts">
	import type { CalendarTimedSegment, CalendarWeekDay } from '$lib/calendar';
	import type {
		CalendarEntry,
		Note,
		Project,
		Task,
		WorkspaceObject,
	} from '@noura/workspace';
	import { CalendarDate, type DateValue } from '@internationalized/date';
	import { Calendar } from '$lib/components/ui/calendar/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Item from '$lib/components/ui/item/index.js';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';

	const HOUR_HEIGHT = 56;
	const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

	let {
		day,
		selectedEntry,
		selectedObject,
		ondatechange,
		onselect,
		onopen,
	}: {
		day: CalendarWeekDay;
		selectedEntry: CalendarEntry | null;
		selectedObject: WorkspaceObject | Task | Note | Project | null;
		ondatechange: (date: Date) => void;
		onselect: (entry: CalendarEntry) => void;
		onopen: (entry: CalendarEntry) => void;
	} = $props();

	const calendarValue = $derived(
		new CalendarDate(
			day.date.getFullYear(),
			day.date.getMonth() + 1,
			day.date.getDate(),
		),
	);
	const heading = $derived(
		day.date.toLocaleDateString(undefined, {
			month: 'long',
			day: 'numeric',
			year: 'numeric',
		}),
	);
	const weekday = $derived(
		day.date.toLocaleDateString(undefined, { weekday: 'long' }),
	);

	const timeFormatter = new Intl.DateTimeFormat(undefined, {
		hour: 'numeric',
		minute: '2-digit',
	});

	function chooseDate(value: DateValue | undefined) {
		if (!value) return;
		ondatechange(new Date(value.year, value.month - 1, value.day));
	}

	function hourLabel(hour: number): string {
		if (hour === 0) return '12 AM';
		if (hour === 12) return '12 PM';
		return `${hour % 12} ${hour < 12 ? 'AM' : 'PM'}`;
	}

	function timeRange(segment: CalendarTimedSegment): string {
		return `${timeFormatter.format(segment.start)}–${timeFormatter.format(segment.end)}`;
	}

	function entryTime(entry: CalendarEntry): string {
		if (entry.allDay) return 'All day';
		const start = new Date(entry.start);
		const end = entry.end ? new Date(entry.end) : null;
		if (Number.isNaN(start.getTime())) return entry.start;
		if (!end || Number.isNaN(end.getTime()) || end <= start)
			return timeFormatter.format(start);
		return `${timeFormatter.format(start)}–${timeFormatter.format(end)}`;
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

	function isSelected(entry: CalendarEntry): boolean {
		return (
			selectedEntry?.sourceId === entry.sourceId &&
			selectedEntry.property === entry.property
		);
	}
</script>

<div class="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_20rem]">
	<section class="flex min-w-0 flex-col">
		<div class="shrink-0 border-b border-border/60 px-5 py-3">
			<h2 class="text-base font-semibold tracking-tight">{heading}</h2>
			<p class="text-sm text-muted-foreground">{weekday}</p>
		</div>

		<div
			class="grid shrink-0 grid-cols-[4rem_minmax(0,1fr)] border-b border-border/60"
		>
			<div class="px-2 py-2 text-right text-xs text-muted-foreground">
				all-day
			</div>
			<div class="flex min-h-12 flex-col gap-1 border-l border-border/60 p-1">
				{#each day.allDayEntries as entry (entry.sourceId + entry.property)}
					<Button
						variant={isSelected(entry) ? 'default' : 'secondary'}
						size="xs"
						class="w-full justify-start overflow-hidden"
						title={entry.title}
						onclick={() => onselect(entry)}
					>
						<span class="truncate">{entry.title}</span>
					</Button>
				{/each}
			</div>
		</div>

		<ScrollArea class="min-h-0 flex-1">
			<div class="grid grid-cols-[4rem_minmax(0,1fr)]">
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
							variant={isSelected(segment.entry) ? 'default' : 'secondary'}
							size="xs"
							class="absolute h-auto min-h-6 w-auto flex-col items-start justify-start gap-0 overflow-hidden rounded-sm text-left"
							style={segmentStyle(segment)}
							title={`${segment.entry.title}, ${timeRange(segment)}`}
							onclick={() => onselect(segment.entry)}
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
			</div>
		</ScrollArea>
	</section>

	<aside class="flex min-h-0 flex-col border-l border-border/60 bg-muted/20">
		<div class="shrink-0 border-b border-border/60 p-2">
			<Calendar
				type="single"
				value={calendarValue}
				placeholder={calendarValue}
				onValueChange={chooseDate}
				weekdayFormat="narrow"
				class="mx-auto w-fit bg-transparent [--cell-size:--spacing(5)] [&>div>div>header]:hidden [&>div>nav]:hidden"
			/>
		</div>

		<div class="min-h-0 flex-1 overflow-auto p-4">
			{#if selectedEntry}
				<div class="flex flex-col gap-4">
					<div>
						<p
							class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
						>
							Event details
						</p>
						<h3 class="mt-1 text-base font-semibold">{selectedEntry.title}</h3>
					</div>
					<Separator />
					<Item.Group class="gap-1">
						<Item.Root size="sm" variant="muted">
							<Item.Content>
								<Item.Title>When</Item.Title>
								<Item.Description>{entryTime(selectedEntry)}</Item.Description>
							</Item.Content>
						</Item.Root>
						<Item.Root size="sm" variant="muted">
							<Item.Content>
								<Item.Title>Source</Item.Title>
								<Item.Description class="capitalize">
									{selectedEntry.sourceType} · {selectedEntry.property}
								</Item.Description>
							</Item.Content>
						</Item.Root>
						{#if selectedObject?.relativePath}
							<Item.Root size="sm" variant="muted">
								<Item.Content>
									<Item.Title>File</Item.Title>
									<Item.Description class="break-all font-mono text-xs">
										{selectedObject.relativePath}
									</Item.Description>
								</Item.Content>
							</Item.Root>
						{/if}
					</Item.Group>
					{#if selectedObject?.body}
						<p
							class="line-clamp-6 text-sm leading-relaxed text-muted-foreground"
						>
							{selectedObject.body}
						</p>
					{/if}
					<Button variant="outline" onclick={() => onopen(selectedEntry)}>
						Open item
					</Button>
				</div>
			{:else}
				<Empty.Root class="h-full border-0 p-4">
					<Empty.Media variant="icon">
						<CalendarBlank />
					</Empty.Media>
					<Empty.Header>
						<Empty.Title>No event selected</Empty.Title>
						<Empty.Description>
							Select an event in the timeline to see its details.
						</Empty.Description>
					</Empty.Header>
				</Empty.Root>
			{/if}
		</div>
	</aside>
</div>
