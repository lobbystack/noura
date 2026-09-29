import type { ManagedDraftInput } from '@noura/workspace';
import {
	AutosaveCoordinator,
	type AutosaveOptions,
	type AutosaveState,
} from './autosave';

/** The editable parts of a managed object such as a task or a project. */
export interface ManagedDraft {
	title: string;
	body: string;
	properties: Record<string, unknown>;
}

/** A managed object as the file holds it. */
export interface ManagedCanonical extends ManagedDraft {
	id: string;
	revision: string;
}

/**
 * What the session needs from the form. The form owns the title, body and
 * properties on screen; the session reads them when saving and moves them
 * to a new version through `rebase`, never by replacing them outright.
 */
export interface ManagedDraftPort {
	/** The draft on screen now. */
	read(): ManagedDraft;
	/**
	 * Show `to` where the screen showed `from`, keeping anything changed
	 * since `from`. `rebaseManagedDraft` does this for the title and
	 * properties; the body goes through the text editor's own rebase.
	 */
	rebase(from: ManagedDraft, to: ManagedDraft): void;
}

export type ManagedSaveResult<Canonical> =
	| { status: 'unchanged'; current: Canonical }
	| {
			status: 'merged';
			current: Canonical;
			title: string;
			body: string;
			properties: Record<string, unknown>;
	  }
	| { status: 'conflict'; current: Canonical };

export interface ManagedDraftSessionOptions<
	Canonical extends ManagedCanonical,
> {
	base: Canonical;
	/**
	 * Durably write the draft, merging with the file when it changed since
	 * the base. Resolves after the bytes are on disk; failures reject.
	 */
	save(input: ManagedDraftInput): Promise<ManagedSaveResult<Canonical>>;
	/** Read the object as the file holds it now. */
	read(): Promise<Canonical>;
	/** A save or reload produced a new canonical version. */
	onCanonical?(canonical: Canonical): void;
	/** Both sides changed the same field; autosave pauses until resolved. */
	onConflict(conflict: { draft: ManagedDraft; file: Canonical }): void;
	/** Changes made outside the app were merged into the draft on screen. */
	onMerged?(): void;
	onStateChange?(state: AutosaveState): void;
	autosave?: Pick<
		AutosaveOptions<true>,
		'schedule' | 'now' | 'debounceDelayMs' | 'maxDelayMs'
	>;
}

function sameValue(left: unknown, right: unknown) {
	return JSON.stringify(left) === JSON.stringify(right);
}

function sameDraft(left: ManagedDraft, right: ManagedDraft) {
	return (
		left.title === right.title &&
		left.body === right.body &&
		sameValue(left.properties, right.properties)
	);
}

function draftOf(value: ManagedDraft): ManagedDraft {
	return {
		title: value.title,
		body: value.body,
		properties: { ...value.properties },
	};
}

/**
 * The title and properties to show when the screen moves from `from` to
 * `to`. A field keeps its on-screen value when it changed since `from`, and
 * takes the value from `to` otherwise. The body is left as it is on screen.
 */
export function rebaseManagedDraft(
	screen: ManagedDraft,
	from: ManagedDraft,
	to: ManagedDraft,
): ManagedDraft {
	const properties: Record<string, unknown> = {};
	const keys = new Set([
		...Object.keys(screen.properties),
		...Object.keys(from.properties),
		...Object.keys(to.properties),
	]);
	for (const key of keys) {
		const edited = !sameValue(screen.properties[key], from.properties[key]);
		const value = edited ? screen.properties[key] : to.properties[key];
		if (value !== undefined) properties[key] = value;
	}
	return {
		title: screen.title === from.title ? to.title : screen.title,
		body: screen.body,
		properties,
	};
}

/**
 * Autosave and reload for one open task or project, following the same
 * rules as `DocumentSession`: every write reads the draft when it starts,
 * and every new version reaches the screen as a rebase of the draft the
 * write (or reload) started from. Edits made while a save or reload is in
 * flight stay on screen and go into the next save, and the base only moves
 * to a version the screen contains, so a later save cannot revert a change
 * made outside the app.
 */
export class ManagedDraftSession<Canonical extends ManagedCanonical> {
	readonly autosave: AutosaveCoordinator<true>;
	#base: Canonical;
	#port: ManagedDraftPort | null = null;
	readonly #options: ManagedDraftSessionOptions<Canonical>;

	constructor(options: ManagedDraftSessionOptions<Canonical>) {
		this.#options = options;
		this.#base = options.base;
		this.autosave = new AutosaveCoordinator<true>({
			...options.autosave,
			write: () => this.#write(),
			...(options.onStateChange
				? { onStateChange: options.onStateChange }
				: {}),
		});
	}

	/** The last version known to be on disk. */
	get base(): Canonical {
		return this.#base;
	}

	/** The draft on screen, or the base while nothing is attached. */
	draft(): ManagedDraft {
		return this.#port?.read() ?? draftOf(this.#base);
	}

	attach(port: ManagedDraftPort | null) {
		this.#port = port;
	}

	/** The save request for `local` on top of the current base. */
	input(local: ManagedDraft): ManagedDraftInput {
		const base = this.#base;
		return {
			id: base.id,
			baseRevision: base.revision,
			baseTitle: base.title,
			baseBody: base.body,
			baseProperties: { ...base.properties },
			localTitle: local.title,
			localBody: local.body,
			localProperties: local.properties,
		};
	}

	/** The user changed the title, body or a property. */
	edited() {
		this.autosave.noteEdit(true);
	}

	get hasPendingWork(): boolean {
		return (
			this.autosave.pendingEdits > 0 ||
			this.autosave.isWriting ||
			this.autosave.error !== null
		);
	}

	flush(): Promise<boolean> {
		return this.autosave.flush();
	}

	/** Hold autosave, for example while the file is gone. */
	pause() {
		this.autosave.pause();
	}

	/**
	 * A conflict was resolved and `canonical` is now on disk. With `show`,
	 * the screen switches to it; otherwise anything edited since the
	 * conflict opened saves on top of it.
	 */
	resolved(canonical: Canonical, show: boolean) {
		const screen = this.draft();
		this.#base = canonical;
		if (show) this.#port?.rebase(screen, draftOf(canonical));
		this.autosave.acceptDurable();
		this.autosave.resume();
		if (!show && !sameDraft(this.draft(), draftOf(canonical))) this.edited();
		this.#options.onCanonical?.(canonical);
	}

	/**
	 * The file changed outside the app. Unsaved edits save now (the save
	 * merges both sides); otherwise the screen follows the file.
	 */
	async externalChange(): Promise<void> {
		// A running save merges the file under the write lock.
		if (this.autosave.isWriting) return;
		if (this.autosave.pendingEdits > 0 || this.autosave.error !== null) {
			await this.flush();
			return;
		}
		const base = this.#base;
		const from = this.draft();
		const latest = await this.#options.read();
		// A save that finished during the read already carried the change.
		if (this.#base !== base || this.autosave.isWriting) return;
		this.#base = latest;
		// Edits made during the read stay; they save on the new base.
		this.#port?.rebase(from, draftOf(latest));
		this.#options.onCanonical?.(latest);
	}

	destroy() {
		this.autosave.destroy();
	}

	async #write(): Promise<void | 'paused'> {
		const local = this.draft();
		const result = await this.#options.save(this.input(local));
		if (result.status === 'conflict') {
			this.#options.onConflict({ draft: local, file: result.current });
			return 'paused';
		}
		const saved =
			result.status === 'merged'
				? {
						title: result.title,
						body: result.body,
						properties: { ...result.properties },
					}
				: draftOf(result.current);
		this.#base = result.current;
		// Only a merged save changes what is on screen. A plain save can
		// normalize the draft (a trailing space, a final blank line), and
		// replacing the fields with that would undo what was just typed.
		if (result.status === 'merged' && !sameDraft(local, saved)) {
			this.#port?.rebase(local, saved);
			if (result.body !== local.body) this.#options.onMerged?.();
		}
		this.#options.onCanonical?.(result.current);
	}
}
