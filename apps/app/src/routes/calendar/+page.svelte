<script lang="ts">
	import { getNouraClient } from '$lib/state.svelte';
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

	let cursor = $state(startOfMonth(today));
	let mode = $state<'month' | 'week' | 'day'>('month');
	let entries = $state<CalendarEntry[]>([]);
	let loading = $state(true);
	let selected = $state<{ id: string; type: string; title: string } | null>(
		null,
	);
	let inspectorOpen = $state(false);
	let selectedObject = $state<WorkspaceObject | Task | Note | Project | null>(
		null,
	);

	const monthLabel = $derived(
		cursor.toLocaleDateString(undefined, { month: 'long', year: 'numeric' }),
	);

	function isoDay(d: Date) {
		const y = d.getFullYear();
		const m = String(d.getMonth() + 1).padStart(2, '0');
		const day = String(d.getDate()).padStart(2, '0');
		return `${y}-${m}-${day}`;
	}

	async function load() {
		try {
			loading = true;
			const start = new SvelteDate(cursor.getFullYear(), cursor.getMonth(), 1);
			start.setDate(start.getDate() - 7);
			const end = new SvelteDate(
				cursor.getFullYear(),
				cursor.getMonth() + 1,
				0,
			);
			end.setDate(end.getDate() + 7);
			entries = await getNouraClient().calendar.queryRange({
				start: start.toISOString(),
				end: end.toISOString(),
			});
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
		cursor = new SvelteDate(
			cursor.getFullYear(),
			cursor.getMonth() + months,
			1,
		);
		load();
	}

	function goToday() {
		cursor = startOfMonth(today);
		load();
	}

	async function openEntry(entry: CalendarEntry) {
		selected = {
			id: entry.sourceId,
			type: entry.sourceType,
			title: entry.title,
		};
		inspectorOpen = true;
		try {
			const client = getNouraClient();
			const obj =
				entry.sourceType === 'task'
					? await client.tasks.get(entry.sourceId)
					: entry.sourceType === 'project'
						? await client.projects.get(entry.sourceId)
						: await client.notes.get(entry.sourceId);
			selectedObject = obj;
		} catch {
			selectedObject = null;
		}
		tabsStore.open(entry.sourceId, entry.sourceType, entry.title);
	}

	onMount(() => {
		if (browser) load();
	});
</script>

<PageHeader title="Calendar" description={monthLabel}>
	{#snippet actions()}
		<div class="flex items-center gap-1">
			<ToggleGroup.Root
				bind:value={
					() => mode,
					(value) => {
						if (value === 'month' || value === 'week' || value === 'day')
							mode = value;
					}
				}
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
{:else if mode !== 'month'}
	<Empty.Root class="flex-1">
		<Empty.Media variant="icon">
			<CalendarBlank />
		</Empty.Media>
		<Empty.Header>
			<Empty.Title>{mode === 'week' ? 'Week' : 'Day'} view</Empty.Title>
			<Empty.Description>
				The agenda views are next. The month grid shows every dated item.
			</Empty.Description>
		</Empty.Header>
	</Empty.Root>
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
