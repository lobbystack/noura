/**
 * Global open state for the command palette (search + quick actions).
 * The rail search button and the Cmd+K handler both drive this store; the
 * dialog component renders it.
 */
class CommandPaletteStore {
	open = $state(false);

	toggle() {
		this.open = !this.open;
	}

	close() {
		this.open = false;
	}

	show() {
		this.open = true;
	}
}

export const commandPalette = new CommandPaletteStore();
