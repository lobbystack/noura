import type { EditorView } from '@codemirror/view';
import type * as Y from 'yjs';

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
}

export interface LiveMarkdownOptions {
	ytext: Y.Text;
	collaborative?: boolean;
	readOnly?: boolean;
	resolveImage?: (src: string) => string | null | Promise<string | null>;
	resolveLink?: (target: string) => Promise<{
		kind: 'managed' | 'markdown' | 'asset' | 'pdf' | 'unresolved';
		relativePath?: string;
		page?: number | null;
		label?: string;
		preview?: string;
	}>;
	openPdf?: (target: { relativePath: string; page?: number | null }) => void;
	mountPdfEmbed?: (
		container: HTMLElement,
		target: { relativePath: string; page?: number | null },
	) => () => void;
	onChange?: (view: EditorView) => void;
	onSelectionChange?: (selection: EditorSelectionState) => void;
}

export interface LiveMarkdownEditor {
	view: EditorView;
	doc: () => string;
	setText: (text: string) => void;
	format: (format: MarkdownFormat) => void;
	undo: () => void;
	redo: () => void;
	destroy: () => void;
	focus: () => void;
}

export interface LiveMarkdownDocument {
	ytext: Y.Text;
	destroy: () => void;
}
