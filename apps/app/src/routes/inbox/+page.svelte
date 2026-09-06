<script lang="ts">
	import { browser } from '$app/environment';
	import { toast } from 'svelte-sonner';
	import { dashboard, daypartGreeting, dueLabel } from '$lib/dashboard.svelte';
	import { LiveProjection } from '$lib/live-refresh';
	import { plugins } from '$lib/plugins.svelte';
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
	import { onMount } from 'svelte';

	let newTaskTitle = $state('');
	let adding = $state(false);
	let completing = $state<string | null>(null);
	let projection = $state.raw<LiveProjection | null>(null);

	const hasAnySection = $derived(
		plugins.isEnabled('tasks') ||
			plugins.isEnabled('calendar') ||
			plugins.isEnabled('notes'),
	);
	const issues = $derived(diagnostics.issues);

	function enabledSections() {
		return {
			tasks: plugins.isEnabled('tasks'),
			calendar: plugins.isEnabled('calendar'),
			notes: plugins.isEnabled('notes'),
		};
	}

	onMount(() => {
		if (!browser) return;
		void diagnostics.refresh();
		const coordinator = new LiveProjection({
			refresh: () => dashboard.refresh(enabledSections()),
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
		});
		projection = coordinator;
		void plugins.init().then(() => coordinator.start());
		return () => {
			coordinator.dispose();
			if (projection === coordinator) projection = null;
		};
	});

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
				error instanceof Error ? error.message : 'Could not create the task',
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
		class="mx-auto flex w-full max-w-2xl flex-col gap-6 p-6 {hasAnySection ||
		issues.length > 0
			? ''
			: 'h-full justify-center'}"
	>
		{#if hasAnySection || issues.length > 0}
			<header>
				<h2 class="text-lg font-semibold tracking-tight">
					{daypartGreeting(new Date())}
				</h2>
				<p class="mt-1 text-sm text-muted-foreground">
					What needs attention and what is happening next.
				</p>
			</header>
		{/if}

		{#if issues.length > 0}
			<section class="flex flex-col gap-3">
				<div class="flex items-center gap-2">
					<h3 class="text-sm font-medium">Needs attention</h3>
					<Badge variant="secondary" class="text-xs">{issues.length}</Badge>
				</div>
				<div class="divide-y divide-border/60 rounded-lg border border-border">
					{#each issues as issue (issue.code)}
						<div class="flex items-start gap-3 px-4 py-2.5">
							<Badge variant="destructive" class="mt-0.5 shrink-0 text-xs"
								>{issue.code}</Badge
							>
							<div class="min-w-0 flex-1">
								<p class="text-sm">{issue.message}</p>
								{#if issue.relativePath}
									<p
										class="mt-0.5 truncate font-mono text-xs text-muted-foreground"
									>
										{issue.relativePath}
									</p>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		{#if hasAnySection}
			<div class="flex flex-col gap-5">
				{#if plugins.isEnabled('tasks')}
					<section class="flex flex-col gap-3">
						<form
							class="flex items-center gap-2"
							onsubmit={submitTask}
							aria-label="Quick add a task"
						>
							<Input
								bind:value={newTaskTitle}
								placeholder="Add a task for today…"
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
								<h3 class="text-sm font-medium">Due today</h3>
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
										<span class="min-w-0 flex-1 truncate text-sm"
											>{task.title}</span
										>
										<Badge variant="outline" class="shrink-0 text-xs">
											{dueLabel(String(task.properties.due), new Date())}
										</Badge>
									</div>
								{/each}
							{/if}
						</div>
					</section>
				{/if}

				{#if plugins.isEnabled('calendar')}
					<section class="flex flex-col gap-3">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-2">
								<CalendarBlank class="size-4 text-muted-foreground" />
								<h3 class="text-sm font-medium">Next 7 days</h3>
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
									No dated items this week.
								</p>
							{:else}
								{#each dashboard.upcoming as entry (entry.sourceId + entry.property)}
									<div class="flex items-center gap-3 px-4 py-2.5">
										<Badge
											variant="outline"
											class="shrink-0 text-xs capitalize"
										>
											{entry.sourceType}
										</Badge>
										<span class="min-w-0 flex-1 truncate text-sm"
											>{entry.title}</span
										>
										<span class="shrink-0 text-xs text-muted-foreground">
											{dueLabel(entry.start, new Date())}
										</span>
									</div>
								{/each}
							{/if}
						</div>
					</section>
				{/if}

				{#if plugins.isEnabled('notes')}
					<section class="flex flex-col gap-3">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-2">
								<NotePencil class="size-4 text-muted-foreground" />
								<h3 class="text-sm font-medium">Recent notes</h3>
							</div>
							<a
								href="/notes"
								class="text-xs text-muted-foreground underline-offset-4 hover:underline"
							>
								All notes
							</a>
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
										href="/notes"
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
					<Empty.Title>Turn on your first modules to get started</Empty.Title>
				</Empty.Header>
				<Empty.Content>
					<Button href="/settings" variant="outline" size="sm">
						Open settings
					</Button>
				</Empty.Content>
			</Empty.Root>
		{/if}
	</div>
</div>
