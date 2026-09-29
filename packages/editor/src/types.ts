import type { EditorView } from '@codemirror/view';

export type MarkdownFormat =
	| 'bold'
	| 'italic'
	| 'underline'
	| 'strikethrough'
	| 'code'
	| 'bulletList'
	| 'numberedList'
	| 'checkList'
	| 'blockQuote'
	| 'link'
	| 'text'
	| 'heading1'
	| 'heading2'
	| 'heading3';

export type MarkdownBlockStyle = 'text' | 'heading1' | 'heading2' | 'heading3';

export interface EditorSelectionState {
	from: number;
	to: number;
	anchor: { left: number; top: number; bottom: number } | null;
	hasFocus: boolean;
	composing: boolean;
	blockStyle: MarkdownBlockStyle;
	/** Inline and line formats in effect at the selection. */
	formats: MarkdownFormat[];
}

export type LinkKind = 'wiki' | 'markdown' | 'url';

export interface LinkOpenOptions {
	kind: LinkKind;
	/** Cmd/Ctrl-click asks for a new tab. */
	newTab: boolean;
}

/** One `[[` completion. */
export interface LinkSuggestion {
	/** Text inserted between `[[` and `]]`. */
	target: string;
	/** Name shown in the list. */
	label: string;
	/** Folder or other context shown next to the name. */
	detail?: string | undefined;
}

/** Restorable caret and scroll position for one document. */
export interface EditorViewMemory {
	anchor: number;
	head: number;
	/** Document position at the top of the scrolled view. */
	top: number;
}

export interface LiveMarkdownOptions {
	/** The text the editor starts with. */
	text: string;
	/** Accessible name of the editable text area. */
	label?: string;
	readOnly?: boolean;
	resolveImage?: (src: string) => string | null | Promise<string | null>;
	/**
	 * Download a remote image the user asked to load and return a URL the
	 * page may show, such as a data URL. Without it the image loads directly.
	 */
	loadRemoteImage?: (url: string) => Promise<string>;
	resolveLink?: (target: string) => Promise<{
		kind: 'managed' | 'markdown' | 'asset' | 'pdf' | 'unresolved';
		relativePath?: string;
		page?: number | null;
		label?: string;
		preview?: string;
	}>;
	/** Follow a link: plain click on rendered links, Cmd/Ctrl-click anywhere. */
	openLink?: (target: string, options: LinkOpenOptions) => void;
	/** Candidates for `[[` completion. */
	linkSuggestions?: () => readonly LinkSuggestion[];
	openPdf?: (target: { relativePath: string; page?: number | null }) => void;
	mountPdfEmbed?: (
		container: HTMLElement,
		target: { relativePath: string; page?: number | null },
	) => () => void;
	/** Called after every user edit. Read the text with `doc()` when needed. */
	onChange?: (view: EditorView) => void;
	onSelectionChange?: (selection: EditorSelectionState) => void;
	/** Let the platform underline misspelled words. Off by default. */
	spellcheck?: boolean;
	/** Caret and scroll position to restore. */
	restore?: EditorViewMemory | null | undefined;
}

export interface LiveMarkdownEditor {
	view: EditorView;
	/** The current text. Converts the whole document; avoid per keystroke. */
	doc: () => string;
	/**
	 * Show a new canonical text. Only the changed ranges are replaced, so the
	 * caret stays where it was, and the change is not added to undo history.
	 */
	setText: (text: string) => void;
	/**
	 * Apply the changes that turned `base` into `target` on top of whatever
	 * the user typed since the editor showed `base`. Nothing typed is lost.
	 */
	rebase: (base: string, target: string) => void;
	format: (format: MarkdownFormat) => void;
	undo: () => void;
	redo: () => void;
	destroy: () => void;
	focus: () => void;
	setSpellcheck: (enabled: boolean) => void;
	/** Caret and scroll position, for restoring when the file reopens. */
	memory: () => EditorViewMemory;
	/** Open the find and replace panel. */
	find: () => void;
}
