import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
import type {
	BrowserSyncController,
	BrowserSyncStatus,
	BrowserSyncWorkspaceSummary,
} from '$lib/browser-sync';
import { errorText } from './copy';

export interface BrowserSyncWorkspace {
	workspaceId: string | null;
	workspaceFiles: BrowserWorkspaceFiles | null;
}

/**
 * Shared view state for the browser sync settings sections. It mirrors what
 * the controller reports and holds the page-level notice and error. The
 * controller does the work.
 */
export class BrowserSyncPanel {
	controller = $state<BrowserSyncController | null>(null);
	status = $state<BrowserSyncStatus>('unavailable');
	deviceId = $state<string | null>(null);
	enrolled = $state(false);
	summary = $state<BrowserSyncWorkspaceSummary | null>(null);
	busy = $state(false);
	notice = $state('');
	error = $state('');
	/** Increments on lock so sections drop their local state. */
	session = $state(0);

	readonly unlocked = $derived(
		this.status === 'unlocked' || this.status === 'enrolled',
	);

	readonly #workspace: () => BrowserSyncWorkspace;

	constructor(workspace: () => BrowserSyncWorkspace) {
		this.#workspace = workspace;
	}

	get workspaceId(): string | null {
		return this.#workspace().workspaceId;
	}

	get workspaceFiles(): BrowserWorkspaceFiles | null {
		return this.#workspace().workspaceFiles;
	}

	/** Attach the controller and pick up an existing workspace binding. */
	async connect(controller: BrowserSyncController): Promise<void> {
		this.controller = controller;
		controller.setBinding(null);
		await this.resume();
		await this.refresh();
	}

	/** Reload a saved binding for the open workspace, if this browser is set up. */
	async resume(): Promise<void> {
		const controller = this.controller;
		const { workspaceId, workspaceFiles } = this.#workspace();
		if (!controller || !workspaceId || !workspaceFiles) return;
		if (!controller.device()?.enrolled) return;
		await controller.resumeSync({ workspaceId, workspaceFiles });
	}

	async refresh(): Promise<void> {
		const controller = this.controller;
		if (!controller) return;
		this.status = controller.status();
		const device = controller.device();
		this.deviceId = device?.deviceId ?? null;
		this.enrolled = device?.enrolled ?? false;
		this.summary = await controller.workspaceSummary();
	}

	/**
	 * Run one device or sync action. The action returns a notice on success or
	 * sets `error` itself. The panel refreshes afterward.
	 */
	async run(
		action: (controller: BrowserSyncController) => Promise<string | void>,
	): Promise<void> {
		const controller = this.controller;
		if (!controller || this.busy) return;
		this.busy = true;
		this.error = '';
		this.notice = '';
		try {
			const notice = await action(controller);
			if (notice) this.notice = notice;
		} catch (cause) {
			this.error = errorText(cause, 'Sync failed.');
		} finally {
			this.busy = false;
			await this.refresh();
		}
	}

	lock(): void {
		const controller = this.controller;
		if (!controller || this.busy) return;
		controller.lock();
		this.error = '';
		this.notice = 'Locked. This tab no longer holds the key.';
		this.session += 1;
		void this.refresh();
	}
}
