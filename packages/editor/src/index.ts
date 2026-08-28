import StarterKit from '@tiptap/starter-kit';
import Collaboration from '@tiptap/extension-collaboration';
import * as Y from 'yjs';

export function createHeadlessEditorFoundation(document = new Y.Doc()) {
	return {
		document,
		extensions: [
			StarterKit.configure({ undoRedo: false }),
			Collaboration.configure({ document }),
		],
	};
}
export interface MarkdownSafety {
	requiresSourceMode: boolean;
	reasons: string[];
}
export function analyzeMarkdownSafety(markdown: string): MarkdownSafety {
	const reasons: string[] = [];
	if (/<[^>]+>/.test(markdown)) reasons.push('inline-html');
	if (/^\s*:::/m.test(markdown)) reasons.push('container-directive');
	if (/^\s*\[[^\]]+\]:\s+/m.test(markdown))
		reasons.push('reference-definition');
	return { requiresSourceMode: reasons.length > 0, reasons };
}
export function detectSuspiciousShrink(
	before: string,
	after: string,
	threshold = 0.35,
): boolean {
	return before.length >= 200 && after.length < before.length * (1 - threshold);
}
