import {
	AutosaveCoordinator,
	type AutosaveOptions,
	type AutosaveState,
} from './autosave';

/**
 * What a document session needs from the text editor. The editor owns the
 * text; the session only reads it when saving and pushes canonical text
 * through explicit calls, never through a reactive prop.
 */
export interface EditorPort {
	/** The text on screen now. */
	text(): string;
	/** Show canonical text, keeping the caret and earlier undo history. */
	setText(text: string): void;
	/** Apply `base` → `target` on top of whatever was typed since `base`. */
	rebase(base: string, target: string): void;
}

/** The last text known to be on disk, with its revision. */
export interface TextBase {
	revision: string;
	body: string;
}

export type SessionSaveResult<Canonical> =
	| { status: 'saved'; base: TextBase; canonical: Canonical }
	| { status: 'conflict'; current: Canonical };

export interface DocumentSessionOptions<Canonical> {
	base: TextBase;
	/**
	 * Durably write `body`, merging with the file when it changed since
	 * `base`. Resolves after the bytes are on disk; failures reject.
	 */
	save(base: TextBase, body: string): Promise<SessionSaveResult<Canonical>>;
	/** Read the file as it is now. */
	read(): Promise<{ base: TextBase; canonical: Canonical }>;
	/** A save or reload produced a new canonical version. */
	onCanonical?(canonical: Canonical): void | Promise<void>;
	/** Both sides changed the same text; autosave is paused until resolved. */
	onConflict(conflict: { localBody: string; current: Canonical }): void;
	/** Changes made outside the editor were merged into the text on screen. */
	onMerged?(): void;
	onStateChange?(state: AutosaveState): void;
	/** True while another flow owns the file; writes pause meanwhile. */
	blocked?(): boolean;
	autosave?: Pick<
		AutosaveOptions<true>,
		'schedule' | 'now' | 'debounceDelayMs' | 'maxDelayMs'
	>;
}

/**
 * Autosave and reload for one open document.
 *
 * Every write reads the editor text when it starts, and every canonical
 * version reaches the editor as a rebase of the text the write (or reload)
 * started from. Keystrokes typed while a save or reload is in flight
 * therefore stay on screen and in the next save, and the base only moves to
 * a version the editor text actually contains, so a later save can never
 * silently revert a change made outside the app.
 */
export class DocumentSession<Canonical> {
	readonly autosave: AutosaveCoordinator<true>;
	#base: TextBase;
	#editor: EditorPort | null = null;
	#discarded = false;
	readonly #options: DocumentSessionOptions<Canonical>;

	constructor(options: DocumentSessionOptions<Canonical>) {
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

	get base(): TextBase {
		return this.#base;
	}

	/** The text on screen, or the base while no editor is attached. */
	text(): string {
		return this.#editor?.text() ?? this.#base.body;
	}

	attach(editor: EditorPort | null) {
		this.#editor = editor;
		// The editor mounts with the text it was given at render time. If a
		// reload landed before it was ready, show that version; nobody can
		// have typed into it yet.
		if (editor && editor.text() !== this.#base.body && !this.hasPendingWork) {
			editor.setText(this.#base.body);
		}
	}

	/** The user changed the text. */
	edited() {
		if (this.#discarded) return;
		this.autosave.noteEdit(true);
	}

	/** Work that must finish (or be discarded) before the editor closes. */
	get hasPendingWork(): boolean {
		if (this.#discarded) return false;
		return (
			this.autosave.pendingEdits > 0 ||
			this.autosave.isWriting ||
			this.autosave.error !== null ||
			(this.#options.blocked?.() ?? false)
		);
	}

	flush(): Promise<boolean> {
		if (this.#discarded) return Promise.resolve(true);
		return this.autosave.flush();
	}

	/**
	 * Run a file operation (such as a rename) with every edit saved first
	 * and autosave held until it finishes. Edits typed meanwhile save after.
	 */
	async exclusive<T>(operation: () => Promise<T>): Promise<T> {
		if (!(await this.flush())) {
			throw new Error('Save your changes before renaming.');
		}
		this.autosave.pause();
		try {
			return await operation();
		} finally {
			this.autosave.resume();
		}
	}

	/** A new canonical version whose text matches the base (e.g. a rename). */
	adoptRevision(base: TextBase) {
		if (base.body === this.#base.body) {
			this.#base = base;
			return;
		}
		this.#editor?.rebase(this.#base.body, base.body);
		this.#base = base;
	}

	/**
	 * A conflict was resolved and `base` is now on disk. With `show`, the
	 * editor switches to that text; otherwise anything typed since the
	 * conflict opened is saved on top of it.
	 */
	resolved(base: TextBase, show: boolean) {
		this.#base = base;
		if (show) this.#editor?.setText(base.body);
		this.autosave.acceptDurable();
		this.autosave.resume();
		if (!show && this.text() !== base.body) this.edited();
	}

	/** Drop unsaved changes so the editor can close. */
	discard() {
		this.#discarded = true;
		this.autosave.acceptDurable();
		this.autosave.pause();
	}

	/**
	 * The file changed outside the editor. Unsaved edits are saved now (the
	 * save merges both sides); otherwise the editor follows the file.
	 */
	async externalChange(): Promise<void> {
		if (this.#discarded || this.#options.blocked?.()) return;
		// A running save reads the file under the write lock and merges it.
		if (this.autosave.isWriting) return;
		if (this.autosave.pendingEdits > 0 || this.autosave.error !== null) {
			await this.flush();
			return;
		}
		const base = this.#base;
		const latest = await this.#options.read();
		// A save that finished during the read already carried the change.
		if (
			this.#discarded ||
			this.#base !== base ||
			this.autosave.isWriting ||
			this.#options.blocked?.()
		) {
			return;
		}
		this.#base = latest.base;
		if (latest.base.body !== base.body) {
			// Anything typed during the read stays; it saves on the new base.
			this.#editor?.rebase(base.body, latest.base.body);
		}
		await this.#options.onCanonical?.(latest.canonical);
	}

	destroy() {
		this.autosave.destroy();
	}

	async #write(): Promise<void | 'paused'> {
		const options = this.#options;
		if (options.blocked?.()) return 'paused';
		const base = this.#base;
		const body = this.text();
		const result = await options.save(base, body);
		if (options.blocked?.()) return 'paused';
		if (result.status === 'conflict') {
			options.onConflict({ localBody: body, current: result.current });
			return 'paused';
		}
		this.#base = result.base;
		if (result.base.body !== body) {
			this.#editor?.rebase(body, result.base.body);
			options.onMerged?.();
		}
		await options.onCanonical?.(result.canonical);
	}
}
