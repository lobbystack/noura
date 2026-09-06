<script lang="ts">
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import {
		calendarDayRange,
		calendarRange,
		calendarWeekRange,
		projectCalendarDay,
		projectCalendarWeek,
	} from '$lib/calendar';
	import { LiveProjection } from '$lib/live-refresh';
	import CalendarWeek from '$lib/components/calendar-week.svelte';
	import CalendarDayView from '$lib/components/calendar-day-view.svelte';
	import type { Note, Project, Task, WorkspaceObject } from '@noura/workspace';
	import { tabsStore } from '$lib/tabs.svelte';
	import ObjectInspector from '$lib/components/object-inspector.svelte';
	import PageHeader from '$lib/components/page-header.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as ToggleGroup from '$lib/components/ui/toggle-group/index.js';
	import CaretLeft from 'phosphor-svelte/lib/CaretLeft';
	import CaretRight from 'phosphor-svelte/lib/CaretRight';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
	import { onMount } from 'svelte';
	import { SvelteDate, SvelteMap } from 'svelte/reactivity';
	import { browser } from '$app/environment';

	type CalendarEntry = Awaited<
		ReturnType<ReturnType<typeof getNouraClient>['calendar']['queryRange']>
	>[number];

	type Cell = {
		date: Date;
		inMonth: boolean;
		isToday: boolean;
		entries: CalendarEntry[];
	};

	const today = new SvelteDate();
	const startOfMonth = (d: Date) =>
		new SvelteDate(d.getFullYear(), d.getMonth(), 1);

	let cursor = new SvelteDate(
		today.getFullYear(),
		today.getMonth(),
		today.getDate(),
	);
	let mode = $state<'month' | 'week' | 'day'>('month');
	let entries = $state<CalendarEntry[]>([]);
	let loading = $state(true);
	let initialError = $state<string | null>(null);
	let selected = $state<{ id: string; type: string; title: string } | null>(
		null,
	);
	let inspectorOpen = $state(false);
	let selectedEntry = $state<CalendarEntry | null>(null);
	let selectedObject = $state<WorkspaceObject | Task | Note | Project | null>(
		null,
	);
	let projection = $state.raw<LiveProjection | null>(null);

	const monthLabel = $derived(
		cursor.toLocaleDateString(undefined, { month: 'long', year: 'numeric' }),
	);
	const weekDays = $derived(projectCalendarWeek(entries, cursor));
	const day = $derived(projectCalendarDay(entries, cursor));

	function isoDay(d: Date) {
		const y = d.getFullYear();
		const m = String(d.getMonth() + 1).padStart(2, '0');
		const day = String(d.getDate()).padStart(2, '0');
		return `${y}-${m}-${day}`;
	}

	function errorMessage(error: unknown): string {
		return error instanceof Error
			? error.message
			: 'Could not load the calendar';
	}

	async function load() {
		try {
			loading = true;
			const range =
				mode === 'week'
					? calendarWeekRange(cursor)
					: mode === 'day'
						? calendarDayRange(cursor)
						: calendarRange(cursor);
			entries = await getNouraClient().calendar.queryRange(range);
			initialError = null;
		} catch (error) {
			if (entries.length === 0) initialError = errorMessage(error);
		} finally {
			loading = false;
		}
	}

	const byDay = $derived.by(() => {
		const map = new SvelteMap<string, CalendarEntry[]>();
		for (const entry of entries) {
			const key = entry.start.slice(0, 10);
			const list = map.get(key) ?? [];
			list.push(entry);
			map.set(key, list);
		}
		return map;
	});

	const monthCells = $derived.by<Cell[]>(() => {
		const first = startOfMonth(cursor);
		const startWeekday = first.getDay();
		const gridStart = new SvelteDate(first);
		gridStart.setDate(first.getDate() - startWeekday);
		const cells: Cell[] = [];
		for (let i = 0; i < 42; i++) {
			const d = new SvelteDate(gridStart);
			d.setDate(gridStart.getDate() + i);
			cells.push({
				date: d,
				inMonth: d.getMonth() === cursor.getMonth(),
				isToday: isoDay(d) === isoDay(today),
				entries: byDay.get(isoDay(d)) ?? [],
			});
		}
		return cells;
	});

	function shift(months: number) {
		if (mode === 'week' || mode === 'day') {
			cursor.setDate(cursor.getDate() + months * (mode === 'week' ? 7 : 1));
			clearDaySelection();
			void load();
			return;
		}
		cursor.setFullYear(cursor.getFullYear(), cursor.getMonth() + months, 1);
		void load();
	}

	function goToday() {
		cursor.setFullYear(today.getFullYear(), today.getMonth(), today.getDate());
		clearDaySelection();
		void load();
	}

	function clearDaySelection() {
		selectedEntry = null;
		selectedObject = null;
		selected = null;
	}

	function selectDate(date: Date) {
		cursor.setFullYear(date.getFullYear(), date.getMonth(), date.getDate());
		clearDaySelection();
		void load();
	}

	function setMode(value: string | undefined) {
		if (
			(value !== 'month' && value !== 'week' && value !== 'day') ||
			value === mode
		)
			return;
		mode = value;
		clearDaySelection();
		void load();
	}

	async function loadEntryObject(
		entry: CalendarEntry,
	): Promise<WorkspaceObject | Task | Note | Project | null> {
		try {
			const client = getNouraClient();
			return entry.sourceType === 'task'
				? await client.tasks.get(entry.sourceId)
				: entry.sourceType === 'project'
					? await client.projects.get(entry.sourceId)
					: await client.notes.get(entry.sourceId);
		} catch {
			return null;
		}
	}

	async function selectDayEntry(entry: CalendarEntry) {
		selectedEntry = entry;
		selectedObject = null;
		const object = await loadEntryObject(entry);
		if (
			selectedEntry?.sourceId === entry.sourceId &&
			selectedEntry.property === entry.property
		)
			selectedObject = object;
	}

	async function openEntry(entry: CalendarEntry) {
		selected = {
			id: entry.sourceId,
			type: entry.sourceType,
			title: entry.title,
		};
		inspectorOpen = true;
		selectedObject = await loadEntryObject(entry);
		tabsStore.open(entry.sourceId, entry.sourceType, entry.title);
	}

	onMount(() => {
		if (!browser) return;
		const coordinator = new LiveProjection({
			refresh: load,
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
			onError: (error) => {
				if (entries.length === 0) initialError = errorMessage(error);
			},
		});
		projection = coordinator;
		void coordinator.start().catch(() => {});
		return () => {
			coordinator.dispose();
			if (projection === coordinator) projection = null;
		};
	});
</script>

<PageHeader title="Calendar" description={monthLabel}>
	{#snippet actions()}
		<div class="flex items-center gap-1">
			<ToggleGroup.Root
				bind:value={() => mode, setMode}
				type="single"
				variant="outline"
				size="sm"
			>
				<ToggleGroup.Item value="month">Month</ToggleGroup.Item>
				<ToggleGroup.Item value="week">Week</ToggleGroup.Item>
				<ToggleGroup.Item value="day">Day</ToggleGroup.Item>
			</ToggleGroup.Root>
			<Button
				variant="ghost"
				size="icon-sm"
				onclick={() => shift(-1)}
				aria-label="Previous month"
			>
				<CaretLeft />
			</Button>
			<Button variant="ghost" size="sm" onclick={goToday}>Today</Button>
			<Button
				variant="ghost"
				size="icon-sm"
				onclick={() => shift(1)}
				aria-label="Next month"
			>
				<CaretRight />
			</Button>
		</div>
	{/snippet}
</PageHeader>

{#if loading}
	<div class="flex-1 p-2">
		<Skeleton class="h-6 w-full" />
		<div class="mt-2 grid grid-cols-7 gap-px">
			{#each Array(35) as _, i (i)}
				<Skeleton class="h-20" />
			{/each}
		</div>
	</div>
{:else if initialError}
	<Empty.Root class="flex-1">
		<Empty.Media variant="icon">
			<CalendarBlank />
		</Empty.Media>
		<Empty.Header>
			<Empty.Title>Calendar unavailable</Empty.Title>
			<Empty.Description>{initialError}</Empty.Description>
		</Empty.Header>
		<Empty.Content>
			<Button onclick={() => void projection?.refreshNow()}>Retry</Button>
		</Empty.Content>
	</Empty.Root>
{:else if mode === 'week'}
	<CalendarWeek days={weekDays} {today} onopen={openEntry} />
{:else if mode === 'day'}
	<CalendarDayView
		{day}
		{selectedEntry}
		{selectedObject}
		ondatechange={selectDate}
		onselect={selectDayEntry}
		onopen={openEntry}
	/>
{:else}
	<div class="flex-1 overflow-auto border-t border-border/60">
		<div class="grid grid-cols-7 border-b border-border/60 bg-muted/40">
			{#each ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'] as day (day)}
				<div
					class="px-2 py-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground"
				>
					{day}
				</div>
			{/each}
		</div>
		<div class="grid grid-cols-7 gap-px bg-border/50">
			{#each monthCells as cell (isoDay(cell.date))}
				<div
					class={`min-h-24 bg-background p-1.5 ${cell.inMonth ? '' : 'bg-muted/20 text-muted-foreground'}`}
				>
					<div class="flex items-center justify-between px-1">
						<span
							class={`flex size-6 items-center justify-center rounded-full text-xs ${cell.isToday ? 'bg-primary font-semibold text-primary-foreground' : 'text-muted-foreground'}`}
						>
							{cell.date.getDate()}
						</span>
					</div>
					<div class="mt-1 flex flex-col gap-0.5">
						{#each cell.entries.slice(0, 3) as entry (entry.sourceId + entry.property)}
							<button
								class="w-full truncate rounded px-1.5 py-0.5 text-left text-[11px] hover:bg-muted"
								title={entry.title}
								onclick={() => openEntry(entry)}
							>
								{#if entry.sourceType}
									<Badge
										variant="outline"
										class="mr-1 px-0.5 text-[9px] leading-none"
									>
										{entry.sourceType}
									</Badge>
								{/if}
								{entry.title}
							</button>
						{/each}
						{#if cell.entries.length > 3}
							<span class="px-1.5 text-[10px] text-muted-foreground">
								+{cell.entries.length - 3} more
							</span>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	</div>
{/if}

<ObjectInspector
	bind:open={inspectorOpen}
	object={selectedObject}
	onclose={() => {
		selected = null;
		selectedObject = null;
	}}
/>
