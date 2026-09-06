import { browser } from '$app/environment';
import type { Project, Task } from '@noura/workspace';
import { LiveProjection } from './live-refresh';
import { getNouraClient, workspace } from './state.svelte';

/**
 * Shared task and project projection for the tasks page and the tasks
 * sidebar section. One fetch, one event loop: both consumers render from
 * this store so the sidebar counts and the page list can never disagree.
 */
class TasksDataStore {
	tasks = $state<Task[]>([]);
	projects = $state<Project[]>([]);
	loading = $state(false);

	#projection: LiveProjection | undefined;
	#loadSequence = 0;

	async start(): Promise<void> {
		if (!browser) return;
		this.#projection ??= new LiveProjection({
			refresh: () => this.load(),
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
		});
		await this.#projection.start();
	}

	async load(): Promise<void> {
		if (!browser) return;
		const sequence = ++this.#loadSequence;
		const workspaceId = workspace.state?.workspaceId;
		this.loading = true;
		try {
			const client = getNouraClient();
			const [taskList, projectList] = await Promise.all([
				client.tasks.list(),
				client.projects.list(),
			]);
			if (
				sequence === this.#loadSequence &&
				workspace.state?.workspaceId === workspaceId
			) {
				this.tasks = taskList;
				this.projects = projectList;
			}
		} catch {
			// Keep the last projection on transient failures; the workspace
			// onboarding flow owns the no-workspace experience.
		} finally {
			if (sequence === this.#loadSequence) this.loading = false;
		}
	}
}

export const tasksData = new TasksDataStore();
