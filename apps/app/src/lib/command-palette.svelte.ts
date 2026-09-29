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

	toggle() {
		if (this.open) this.close();
		else this.show();
	}

	close() {
		this.open = false;
	}

	show(mode: CommandPaletteMode = 'all') {
		this.mode = mode;
		this.open = true;
	}
}

export const commandPalette = new CommandPaletteStore();
