import { browser } from '$app/environment';
import type { CalendarEntry, Note, Task } from '@noura/workspace';
import { filterTasks } from './tasks/filters';
import { addCalendarDays, formatCalendarBoundary } from './calendar';
import { getNouraClient, workspace } from './state.svelte';

export { daypartGreeting, dueLabel } from './dashboard-dates';

export interface DashboardSectionState {
	tasks: boolean;
	calendar: boolean;
	notes: boolean;
}

const TODAY_LIMIT = 6;
const UPCOMING_LIMIT = 5;
const RECENT_LIMIT = 5;

/**
 * Projects the workspace into the Inbox dashboard. Every read goes through
 * the typed client; per-plugin gating decides which reads happen at all.
 * Failures degrade a section to empty instead of failing the page.
 */
class DashboardStore {
	todayTasks = $state<Task[]>([]);
	upcoming = $state<CalendarEntry[]>([]);
	recentNotes = $state<Note[]>([]);
	loading = $state(false);
	error = $state<string | null>(null);
	#refreshSequence = 0;

	async refresh(enabled: DashboardSectionState): Promise<void> {
		if (!browser) return;
		const sequence = ++this.#refreshSequence;
		const workspaceId = workspace.state?.workspaceId;
		this.loading = true;
		const client = getNouraClient();
		const now = new Date();
		const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
		const [tasks, calendar, notes] = await Promise.all([
			enabled.tasks
				? client.tasks.list().then(
						(value) => ({ ok: true as const, value }),
						(error: unknown) => ({ ok: false as const, error }),
					)
				: null,
			enabled.calendar
				? client.calendar
						.queryRange({
							start: formatCalendarBoundary(today),
							end: formatCalendarBoundary(addCalendarDays(today, 7)),
						})
						.then(
							(value) => ({ ok: true as const, value }),
							(error: unknown) => ({ ok: false as const, error }),
						)
				: null,
			enabled.notes
				? client.notes.list().then(
						(value) => ({ ok: true as const, value }),
						(error: unknown) => ({ ok: false as const, error }),
					)
				: null,
		]);
		if (
			sequence !== this.#refreshSequence ||
			workspace.state?.workspaceId !== workspaceId
		) {
			if (sequence === this.#refreshSequence) this.loading = false;
			return;
		}
		if (tasks?.ok)
			this.todayTasks = filterTasks(tasks.value, { mode: 'today' }, now).slice(
				0,
				TODAY_LIMIT,
			);
		if (calendar?.ok) this.upcoming = calendar.value.slice(0, UPCOMING_LIMIT);
		if (notes?.ok) this.recentNotes = notes.value.slice(0, RECENT_LIMIT);
		const failure = [tasks, calendar, notes].find(
			(result) => result && !result.ok,
		);
		this.error =
			failure && !failure.ok
				? failure.error instanceof Error
					? failure.error.message
					: 'Could not refresh the inbox'
				: null;
		this.loading = false;
	}

	/** Create a task through the tasks plugin command, then refresh Today. */
	async addTask(title: string): Promise<void> {
		if (title.trim().length === 0) return;
		await getNouraClient().commands.execute('tasks.create', { title });
		await this.refresh({ tasks: true, calendar: false, notes: false });
	}

	/** Complete through the plugin command's revision-checked update. */
	async completeTask(task: Task): Promise<void> {
		await getNouraClient().commands.execute('tasks.complete', {
			id: task.id,
			expectedRevision: task.revision,
		});
		await this.refresh({ tasks: true, calendar: false, notes: false });
	}
}

export const dashboard = new DashboardStore();
