<script lang="ts">
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { dashboard, daypartGreeting, dueLabel } from '$lib/dashboard.svelte';
	import { isOverdue } from '$lib/dashboard-dates';
	import { homeIssues } from '$lib/home-issues';
	import { LiveProjection } from '$lib/live-refresh';
	import { fileHref, objectHref } from '$lib/navigation-targets';
	import { plugins } from '$lib/plugins.svelte';
	import { cn } from '$lib/utils';
	import { workspace, diagnostics, getNouraClient } from '$lib/state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import CheckCircle from 'phosphor-svelte/lib/CheckCircle';
	import Circle from 'phosphor-svelte/lib/Circle';
	import CalendarBlank from 'phosphor-svelte/lib/CalendarBlank';
	import NotePencil from 'phosphor-svelte/lib/NotePencil';
	import PuzzlePiece from 'phosphor-svelte/lib/PuzzlePiece';
	import Warning from 'phosphor-svelte/lib/Warning';

	const settings = getSettingsDialog();

	let newTaskTitle = $state('');
	let adding = $state(false);
	let completing = $state<string | null>(null);

	const sections = $derived({
		tasks: plugins.isEnabled('tasks'),
		calendar: plugins.isEnabled('calendar'),
		notes: plugins.isEnabled('notes'),
	});
	const hasAnySection = $derived(
		sections.tasks || sections.calendar || sections.notes,
	);
	const issues = $derived(homeIssues(diagnostics.issues));
	// Changes only when plugins finish loading or a section turns on or off.
	const sectionsKey = $derived(
		plugins.synced
			? `${sections.tasks}:${sections.calendar}:${sections.notes}`
			: null,
	);

	let projection: LiveProjection | null = null;

	// Load once plugins are known, then again only when a section turns on
	// or off. Core events keep the page current in between.
	$effect(() => {
		if (sectionsKey === null) return;
		untrack(() => {
			if (projection) {
				void projection.refreshNow().catch(() => {});
				return;
			}
			projection = new LiveProjection({
				refresh: async () => {
					await Promise.all([
						dashboard.refresh(sections),
						diagnostics.refresh().catch(() => {}),
					]);
				},
				subscribe: (handler) => getNouraClient().events.subscribe(handler),
				workspaceId: () => workspace.state?.workspaceId,
				focusSource: window,
				visibilitySource: document,
			});
			void projection.start().catch(() => {});
		});
	});

	$effect(() => () => projection?.dispose());

	async function submitTask(event: SubmitEvent) {
		event.preventDefault();
		const title = newTaskTitle.trim();
		if (title.length === 0 || adding) return;
		adding = true;
		try {
			await dashboard.addTask(title);
			newTaskTitle = '';
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not add the task',
			);
		} finally {
			adding = false;
		}
	}

	async function complete(taskId: string) {
		if (completing) return;
		const task = dashboard.todayTasks.find((entry) => entry.id === taskId);
		if (!task) return;
		completing = taskId;
		try {
			await dashboard.completeTask(task);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not complete the task',
			);
			await dashboard.refresh({ tasks: true, calendar: false, notes: false });
		} finally {
			completing = null;
		}
	}
</script>

<div class="flex-1 overflow-y-auto">
	<div
		class={cn(
			'mx-auto flex w-full max-w-2xl flex-col gap-6 p-6',
			!hasAnySection && issues.length === 0 && 'h-full justify-center',
		)}
	>
		{#if hasAnySection || issues.length > 0}
			<h1 class="text-base font-semibold tracking-tight">
				{daypartGreeting(new Date())}
			</h1>
		{/if}

		{#if issues.length > 0}
			<section class="flex flex-col gap-3" aria-labelledby="attention-heading">
				<div class="flex items-center gap-2">
					<Warning class="size-4 text-destructive" />
					<h2 id="attention-heading" class="text-sm font-medium">
						Needs attention
					</h2>
				</div>
				<ul class="divide-y divide-border/60 rounded-lg border border-border">
					{#each issues as issue (issue.key)}
						<li class="flex items-start gap-3 px-4 py-2.5">
							<p class="min-w-0 flex-1 text-sm">{issue.message}</p>
							{#if issue.paths.length > 0}
								<a
									href={fileHref(issue.paths[0])}
									class="shrink-0 text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
									>Open file</a
								>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if hasAnySection}
			<div class="flex flex-col gap-5">
				{#if sections.tasks}
					<section class="flex flex-col gap-3" aria-labelledby="today-heading">
						<form
							class="flex items-center gap-2"
							onsubmit={submitTask}
							aria-label="Add a task for today"
						>
							<Input
								bind:value={newTaskTitle}
								placeholder="Add a task for today…"
								aria-label="Task"
								disabled={adding}
							/>
							<Button
								type="submit"
								size="sm"
								disabled={adding || newTaskTitle.trim().length === 0}
							>
								Add
							</Button>
						</form>

						<div class="flex items-center justify-between">
							<div class="flex items-center gap-2">
								<CheckCircle class="size-4 text-muted-foreground" />
								<h2 id="today-heading" class="text-sm font-medium">
									Due today
								</h2>
								{#if dashboard.todayTasks.length > 0}
									<Badge variant="secondary" class="text-xs"
										>{dashboard.todayTasks.length}</Badge
									>
								{/if}
							</div>
							<a
								href="/tasks"
								class="text-xs text-muted-foreground underline-offset-4 hover:underline"
							>
								All tasks
							</a>
						</div>
						<div
							class="divide-y divide-border/60 rounded-lg border border-border"
						>
							{#if dashboard.todayTasks.length === 0}
								<p class="px-4 py-3 text-sm text-muted-foreground">
									Nothing due today.
								</p>
							{:else}
								{#each dashboard.todayTasks as task (task.id)}
									{@const due = String(task.properties.due ?? '')}
									<div class="flex items-center gap-3 px-4 py-2.5">
										<button
											type="button"
											class="shrink-0 text-muted-foreground transition-colors hover:text-primary"
											aria-label="Complete {task.title}"
											disabled={completing !== null}
											onclick={() => complete(task.id)}
										>
											<Circle
												class="size-4"
												weight={completing === task.id ? 'fill' : 'regular'}
											/>
										</button>
										<a
											href={objectHref('task', task.id)}
											class="min-w-0 flex-1 truncate text-sm hover:underline"
											>{task.title}</a
										>
										{#if due}
											<span
												class={cn(
													'shrink-0 text-xs text-muted-foreground',
													isOverdue(due, new Date()) && 'text-destructive',
												)}
											>
												{dueLabel(due, new Date())}
											</span>
										{/if}
									</div>
								{/each}
							{/if}
						</div>
					</section>
				{/if}

				{#if sections.calendar}
					<section
						class="flex flex-col gap-3"
						aria-labelledby="upcoming-heading"
					>
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-2">
								<CalendarBlank class="size-4 text-muted-foreground" />
								<h2 id="upcoming-heading" class="text-sm font-medium">
									Next 7 days
								</h2>
							</div>
							<a
								href="/calendar"
								class="text-xs text-muted-foreground underline-offset-4 hover:underline"
							>
								Calendar
							</a>
						</div>
						<div
							class="divide-y divide-border/60 rounded-lg border border-border"
						>
							{#if dashboard.upcoming.length === 0}
								<p class="px-4 py-3 text-sm text-muted-foreground">
									Nothing scheduled this week.
								</p>
							{:else}
								{#each dashboard.upcoming as entry (entry.sourceId + entry.property)}
									{@const href = objectHref(entry.sourceType, entry.sourceId)}
									<svelte:element
										this={href ? 'a' : 'div'}
										{href}
										class={cn(
											'flex items-center gap-3 px-4 py-2.5',
											href && 'transition-colors hover:bg-accent',
										)}
									>
										<span class="min-w-0 flex-1 truncate text-sm"
											>{entry.title}</span
										>
										<span class="shrink-0 text-xs text-muted-foreground">
											{dueLabel(entry.start, new Date())}
										</span>
									</svelte:element>
								{/each}
							{/if}
						</div>
					</section>
				{/if}

				{#if sections.notes}
					<section class="flex flex-col gap-3" aria-labelledby="recent-heading">
						<div class="flex items-center gap-2">
							<NotePencil class="size-4 text-muted-foreground" />
							<h2 id="recent-heading" class="text-sm font-medium">Recent</h2>
						</div>
						<div
							class="divide-y divide-border/60 rounded-lg border border-border"
						>
							{#if dashboard.recentNotes.length === 0}
								<p class="px-4 py-3 text-sm text-muted-foreground">
									No notes yet.
								</p>
							{:else}
								{#each dashboard.recentNotes as note (note.id)}
									<a
										href={objectHref('note', note.id)}
										class="block px-4 py-2.5 transition-colors hover:bg-accent"
									>
										<span class="block truncate text-sm">{note.title}</span>
									</a>
								{/each}
							{/if}
						</div>
					</section>
				{/if}
			</div>
		{:else}
			<Empty.Root>
				<Empty.Media variant="icon">
					<PuzzlePiece />
				</Empty.Media>
				<Empty.Header>
					<Empty.Title>Nothing here yet</Empty.Title>
					<Empty.Description>
						Turn on tasks, calendar or notes to see them on Home.
					</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button
						onclick={() => settings.show('plugins')}
						variant="outline"
						size="sm"
					>
						Choose plugins
					</Button>
				</Empty.Content>
			</Empty.Root>
		{/if}
	</div>
</div>
