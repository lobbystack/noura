/**
 * What the palette shows: everything, files by name (quick open), or file
 * contents (search).
 */
type CommandPaletteMode = 'all' | 'files' | 'search';

/**
 * Global open state for the command palette (search + quick actions).
 * The rail search button, the menu bar and keyboard shortcuts drive this
 * store; the dialog component renders it.
 */
class CommandPaletteStore {
	open = $state(false);
	mode = $state<CommandPaletteMode>('all');
	/** Where focus was before the palette opened, to return it on close. */
	returnFocus: HTMLElement | null = null;
	#stopTypeAhead: (() => void) | null = null;
	/** Characters typed before the palette's field took focus. */
	#typedAhead = '';
	#receiver: ((text: string) => void) | null = null;

	/**
	 * The palette adds characters typed before its field had focus to its
	 * query through `receive`, including any typed before it mounted.
	 */
	receiveTypedAhead(receive: (text: string) => void): () => void {
		this.#receiver = receive;
		this.#deliver();
		return () => {
			if (this.#receiver === receive) this.#receiver = null;
		};
	}

	#deliver() {
		if (!this.#receiver || !this.#typedAhead) return;
		const text = this.#typedAhead;
		this.#typedAhead = '';
		this.#receiver(text);
	}

	toggle() {
		if (this.open) this.close();
		else this.show();
	}

	close() {
		this.open = false;
		this.#stopTypeAhead?.();
	}

	show(mode: CommandPaletteMode = 'all') {
		this.mode = mode;
		if (!this.open) this.#takeKeys();
		this.open = true;
	}

	/**
	 * The palette's field takes focus a moment after the palette opens, and
	 * people start typing right after the shortcut. Until the field has
	 * focus, the element that had it lets go, so those keys cannot edit the
	 * document behind the palette, and the palette collects them instead.
	 */
	#takeKeys() {
		if (typeof document === 'undefined') return;
		this.#stopTypeAhead?.();
		const active = document.activeElement;
		this.returnFocus =
			active instanceof HTMLElement && active !== document.body ? active : null;
		this.returnFocus?.blur();
		this.#typedAhead = '';
		const collect = (event: KeyboardEvent) => {
			if (event.target !== document.body) return;
			if (event.metaKey || event.ctrlKey || event.altKey) return;
			if (event.key.length !== 1) return;
			event.preventDefault();
			this.#typedAhead += event.key;
			this.#deliver();
		};
		const stop = () => {
			window.removeEventListener('keydown', collect, true);
			document.removeEventListener('focusin', stop, true);
			this.#stopTypeAhead = null;
		};
		window.addEventListener('keydown', collect, true);
		// Once anything takes focus, keys go where they belong again.
		document.addEventListener('focusin', stop, true);
		this.#stopTypeAhead = stop;
	}
}

export const commandPalette = new CommandPaletteStore();
