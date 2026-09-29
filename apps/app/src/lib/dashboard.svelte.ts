import { browser } from '$app/environment';
import type { CalendarEntry, Note, Task } from '@noura/workspace';
import { filterTasks } from './tasks/filters';
import { addCalendarDays, formatCalendarBoundary } from './calendar';
import { getNouraClient, workspace } from './state.svelte';

export { daypartGreeting, dueLabel } from './dashboard-dates';

interface DashboardSectionState {
	tasks: boolean;
	calendar: boolean;
	notes: boolean;
}

/** Home lists titles and dates only, so it never loads document bodies. */
type TaskSummary = Omit<Task, 'body'>;
type NoteSummary = Omit<Note, 'body'>;

const TODAY_LIMIT = 6;
const UPCOMING_LIMIT = 5;
const RECENT_LIMIT = 5;
/**
 * Open tasks due by tomorrow, as the index sees their dates. The client
 * then applies the exact local-time "today" rule, so the query must return
 * a superset; a day of slack covers due times written in other offsets.
 */
const DUE_CANDIDATE_LIMIT = 100;

function localDate(date: Date): string {
	const month = String(date.getMonth() + 1).padStart(2, '0');
	const day = String(date.getDate()).padStart(2, '0');
	return `${date.getFullYear()}-${month}-${day}`;
}

/**
 * Projects the workspace into the Home dashboard. Every read goes through
 * the typed client; per-plugin gating decides which reads happen at all.
 * Failures degrade a section to empty instead of failing the page.
 */
class DashboardStore {
	todayTasks = $state<TaskSummary[]>([]);
	upcoming = $state<CalendarEntry[]>([]);
	recentNotes = $state<NoteSummary[]>([]);
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
		const settle = <T>(request: Promise<T>) =>
			request.then(
				(value) => ({ ok: true as const, value }),
				(error: unknown) => ({ ok: false as const, error }),
			);
		const [tasks, calendar, notes] = await Promise.all([
			enabled.tasks
				? settle(
						client.objects.summaries({
							type: 'task',
							statusNot: 'done',
							dueOnOrBefore: localDate(addCalendarDays(today, 1)),
							order: 'due-asc',
							limit: DUE_CANDIDATE_LIMIT,
						}),
					)
				: null,
			enabled.calendar
				? settle(
						client.calendar.queryRange({
							start: formatCalendarBoundary(today),
							end: formatCalendarBoundary(addCalendarDays(today, 7)),
						}),
					)
				: null,
			enabled.notes
				? settle(
						client.objects.summaries({
							type: 'note',
							order: 'updated-desc',
							limit: RECENT_LIMIT,
						}),
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
			this.todayTasks = filterTasks(
				tasks.value.map((task) => ({ ...task, body: '' }) as Task),
				{ mode: 'today' },
				now,
			).slice(0, TODAY_LIMIT);
		if (calendar?.ok) this.upcoming = calendar.value.slice(0, UPCOMING_LIMIT);
		if (notes?.ok) this.recentNotes = notes.value as NoteSummary[];
		const failure = [tasks, calendar, notes].find(
			(result) => result && !result.ok,
		);
		this.error =
			failure && !failure.ok
				? failure.error instanceof Error
					? failure.error.message
					: 'Could not refresh Home'
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
	async completeTask(task: Pick<Task, 'id' | 'revision'>): Promise<void> {
		await getNouraClient().commands.execute('tasks.complete', {
			id: task.id,
			expectedRevision: task.revision,
		});
		await this.refresh({ tasks: true, calendar: false, notes: false });
	}
}

export const dashboard = new DashboardStore();
