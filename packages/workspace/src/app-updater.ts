/** A signed release newer than the running application. */
export interface PendingAppUpdate {
	version: string;
	notes?: string | undefined;
	/** Downloads and verifies the signed bundle without applying it. */
	download(): Promise<void>;
	/** Applies a downloaded bundle. Windows installers may exit the process. */
	install(): Promise<void>;
}

export interface AppUpdaterAdapter {
	check(): Promise<PendingAppUpdate | null>;
	restart(): Promise<void>;
}

export type AppUpdateState =
	| { status: 'idle' }
	| { status: 'checking' }
	| { status: 'downloading'; version: string }
	| {
			status: 'ready';
			version: string;
			notes?: string | undefined;
			error?: string;
	  }
	| { status: 'installing'; version: string };

export interface AppUpdater {
	readonly state: AppUpdateState;
	/**
	 * Checks for a newer release and downloads it in the background. Failures
	 * return to idle: an offline launch or unreachable release host is not an
	 * error the user needs to act on.
	 */
	check(): Promise<AppUpdateState>;
	/**
	 * Flushes pending canonical-file writes, installs the downloaded update, and
	 * restarts. A failed flush leaves the running version untouched.
	 */
	installAndRestart(): Promise<AppUpdateState>;
}

export function createAppUpdater(
	adapter: AppUpdaterAdapter,
	options: {
		flush: () => Promise<boolean>;
		onChange?: (state: AppUpdateState) => void;
	},
): AppUpdater {
	let state: AppUpdateState = { status: 'idle' };
	let pending: PendingAppUpdate | null = null;

	const set = (next: AppUpdateState) => {
		state = next;
		options.onChange?.(next);
		return next;
	};

	return {
		get state() {
			return state;
		},

		async check() {
			if (state.status !== 'idle') return state;
			set({ status: 'checking' });
			try {
				const update = await adapter.check();
				if (!update) return set({ status: 'idle' });
				set({ status: 'downloading', version: update.version });
				await update.download();
				pending = update;
				return set({
					status: 'ready',
					version: update.version,
					notes: update.notes,
				});
			} catch {
				pending = null;
				return set({ status: 'idle' });
			}
		},

		async installAndRestart() {
			if (state.status !== 'ready' || !pending) return state;
			const ready = state;
			const update = pending;
			if (!(await options.flush())) {
				return set({
					...ready,
					error: 'Save or discard your open drafts, then try again.',
				});
			}
			set({ status: 'installing', version: update.version });
			try {
				await update.install();
				await adapter.restart();
				return state;
			} catch {
				return set({
					status: 'ready',
					version: update.version,
					notes: update.notes,
					error: 'Noura could not install the update. Try again later.',
				});
			}
		},
	};
}
