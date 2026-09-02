import { browser } from '$app/environment';
import type { CoreEvent, Project, Task } from '@noura/workspace';
import { getNouraClient } from './state.svelte';

const REFRESH_DEBOUNCE_MS = 250;
const LIVE_EVENTS = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
	'file:changed',
	'search:index-updated',
]);

/**
 * Shared task and project projection for the tasks page and the tasks
 * sidebar section. One fetch, one event loop: both consumers render from
 * this store so the sidebar counts and the page list can never disagree.
 */
class TasksDataStore {
	tasks = $state<Task[]>([]);
	projects = $state<Project[]>([]);
	loading = $state(false);

	#started = false;
	#refreshTimer: ReturnType<typeof setTimeout> | undefined;

	async start(): Promise<void> {
		if (!browser || this.#started) return;
		this.#started = true;
		await this.load();
		try {
			await getNouraClient().events.subscribe((event: CoreEvent) => {
				if (!LIVE_EVENTS.has(event.type)) return;
				clearTimeout(this.#refreshTimer);
				this.#refreshTimer = setTimeout(() => {
					void this.load();
				}, REFRESH_DEBOUNCE_MS);
			});
		} catch {
			this.#started = false;
		}
	}

	async load(): Promise<void> {
		if (!browser) return;
		this.loading = true;
		try {
			const client = getNouraClient();
			const [taskList, projectList] = await Promise.all([
				client.tasks.list(),
				client.projects.list(),
			]);
			this.tasks = taskList;
			this.projects = projectList;
		} catch {
			// Keep the last projection on transient failures; the workspace
			// onboarding flow owns the no-workspace experience.
		} finally {
			this.loading = false;
		}
	}
}

export const tasksData = new TasksDataStore();
