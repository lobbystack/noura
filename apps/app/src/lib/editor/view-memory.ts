import type { EditorViewMemory } from '@noura/editor/types';

/** Caret and scroll position per open document, kept for the session. */
const memory = new Map<string, EditorViewMemory>();
const LIMIT = 200;

export function rememberView(key: string, value: EditorViewMemory) {
	memory.delete(key);
	memory.set(key, value);
	if (memory.size > LIMIT) {
		const oldest = memory.keys().next().value;
		if (oldest !== undefined) memory.delete(oldest);
	}
}

export function recallView(key: string): EditorViewMemory | null {
	return memory.get(key) ?? null;
}

/** A renamed file keeps its place. */
export function moveViewMemory(from: string, to: string) {
	const value = memory.get(from);
	if (!value) return;
	memory.delete(from);
	memory.set(to, value);
}
