import { browser } from '$app/environment';
import type { CalendarEntry, Note, Task } from '@noura/workspace';
import { filterTasks } from './tasks/filters';
import { getNouraClient } from './state.svelte';
import { plusDays } from './dashboard-dates';

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

	async refresh(enabled: DashboardSectionState): Promise<void> {
		if (!browser) return;
		const client = getNouraClient();
		const now = new Date();
		const jobs: Array<Promise<void>> = [];
		if (enabled.tasks) {
			jobs.push(
				client.tasks
					.list()
					.then((tasks) => {
						this.todayTasks = filterTasks(tasks, { mode: 'today' }, now).slice(
							0,
							TODAY_LIMIT,
						);
					})
					.catch(() => {
						this.todayTasks = [];
					}),
			);
		}
		if (enabled.calendar) {
			jobs.push(
				client.calendar
					.queryRange({
						start: now.toISOString(),
						end: plusDays(now, 7).toISOString(),
					})
					.then((entries) => {
						this.upcoming = entries.slice(0, UPCOMING_LIMIT);
					})
					.catch(() => {
						this.upcoming = [];
					}),
			);
		}
		if (enabled.notes) {
			jobs.push(
				client.notes
					.list()
					.then((notes) => {
						this.recentNotes = notes.slice(0, RECENT_LIMIT);
					})
					.catch(() => {
						this.recentNotes = [];
					}),
			);
		}
		await Promise.all(jobs);
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
