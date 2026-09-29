import { describe, expect, test } from 'bun:test';
import { rebaseText } from '@noura/editor/text-sync';
import {
	DocumentSession,
	type EditorPort,
	type SessionSaveResult,
	type TextBase,
} from './document-session';

/** An editor that behaves like the CodeMirror one: rebase keeps typing. */
class FakeEditor implements EditorPort {
	constructor(public value: string) {}
	text() {
		return this.value;
	}
	setText(text: string) {
		this.value = text;
	}
	rebase(base: string, target: string) {
		this.value = rebaseText(base, this.value, target);
	}
	type(text: string, session: DocumentSession<unknown>) {
		this.value += text;
		session.edited();
	}
}

/** A file on disk with a three-way merge like the native side. */
class FakeFile {
	revision = 1;
	constructor(public body: string) {}
	get base(): TextBase {
		return { revision: String(this.revision), body: this.body };
	}
	writeExternally(body: string) {
		this.body = body;
		this.revision += 1;
	}
	/** How the file stores text; the note format drops trailing blank lines. */
	normalize = (text: string) => text;
	save(base: TextBase, local: string): SessionSaveResult<string> {
		const external = base.revision !== String(this.revision);
		const merged = external ? rebaseText(base.body, local, this.body) : local;
		this.writeExternally(this.normalize(merged));
		return {
			status: 'saved',
			base: this.base,
			canonical: this.body,
			merged: external,
		};
	}
}

function deferred() {
	let resolve!: () => void;
	const promise = new Promise<void>((done) => (resolve = done));
	return { promise, resolve };
}

function setup(initial: string) {
	const file = new FakeFile(initial);
	const editor = new FakeEditor(initial);
	let gate: Promise<void> | null = null;
	let readGate: Promise<void> | null = null;
	const saved: string[] = [];
	const merges = { count: 0 };
	const session = new DocumentSession<string>({
		base: file.base,
		// Timers never fire on their own; tests flush explicitly.
		autosave: { schedule: () => () => {} },
		save: async (base, body) => {
			if (gate) await gate;
			const result = file.save(base, body);
			saved.push(file.body);
			return result;
		},
		read: async () => {
			const snapshot = { base: file.base, canonical: file.body };
			if (readGate) await readGate;
			return snapshot;
		},
		onConflict: () => {
			throw new Error('unexpected conflict');
		},
		onMerged: () => {
			merges.count += 1;
		},
	});
	session.attach(editor);
	return {
		file,
		editor,
		session,
		saved,
		merges,
		holdSaves() {
			const hold = deferred();
			gate = hold.promise;
			return () => {
				gate = null;
				hold.resolve();
			};
		},
		holdReads() {
			const hold = deferred();
			readGate = hold.promise;
			return () => {
				readGate = null;
				hold.resolve();
			};
		},
	};
}

describe('DocumentSession', () => {
	test('keystrokes typed during a save stay on screen and are saved next', async () => {
		const { editor, session, file, holdSaves } = setup('Hello');
		editor.type(' wor', session);
		const release = holdSaves();
		const flushing = session.flush();
		await Promise.resolve();
		editor.type('ld', session);
		release();
		await flushing;
		expect(editor.value).toBe('Hello world');
		await session.flush();
		expect(file.body).toBe('Hello world');
	});

	test('a merge during a save cannot revert the external edit later', async () => {
		const { editor, session, file, holdSaves } = setup('line\n');
		editor.type('one', session);
		const release = holdSaves();
		const flushing = session.flush();
		await Promise.resolve();
		// Another app edits the file while the save waits, and the user
		// keeps typing.
		file.writeExternally('outside\nline\n');
		editor.type(' two', session);
		release();
		await flushing;
		// The merged external line is on screen with everything typed.
		expect(editor.value).toBe('outside\nline\none two');
		await session.flush();
		expect(file.body).toBe('outside\nline\none two');
	});

	test('a save that normalizes the text leaves what was typed on screen', async () => {
		const { editor, session, file, merges } = setup('Hello');
		// Like the note format, which ends the file with one newline.
		file.normalize = (text) => text.replace(/\s+$/, '');
		editor.type(' ', session);
		await session.flush();
		expect(file.body).toBe('Hello');
		expect(editor.value).toBe('Hello ');
		editor.type('\n', session);
		await session.flush();
		expect(editor.value).toBe('Hello \n');
		editor.type('world', session);
		await session.flush();
		expect(editor.value).toBe('Hello \nworld');
		expect(file.body).toBe('Hello \nworld');
		expect(merges.count).toBe(0);
	});

	test('a reload keeps text typed while the file was being read', async () => {
		const { editor, session, file, holdReads } = setup('a\n');
		file.writeExternally('a\nfrom outside\n');
		const release = holdReads();
		const reloading = session.externalChange();
		await Promise.resolve();
		editor.type('b', session);
		release();
		await reloading;
		expect(editor.value).toBe('a\nfrom outside\nb');
		await session.flush();
		expect(file.body).toBe('a\nfrom outside\nb');
	});

	test('an external change with unsaved edits saves them merged', async () => {
		const { editor, session, file } = setup('a\n');
		editor.type('mine', session);
		file.writeExternally('theirs\na\n');
		await session.externalChange();
		expect(file.body).toBe('theirs\na\nmine');
		expect(editor.value).toBe('theirs\na\nmine');
	});

	test('a clean reload follows the file', async () => {
		const { editor, session, file } = setup('a\n');
		file.writeExternally('b\n');
		await session.externalChange();
		expect(editor.value).toBe('b\n');
		expect(session.hasPendingWork).toBe(false);
	});

	test('discarding drops unsaved work so the editor can close', async () => {
		const { editor, session, file } = setup('a');
		editor.type('b', session);
		expect(session.hasPendingWork).toBe(true);
		session.discard();
		expect(session.hasPendingWork).toBe(false);
		await expect(session.flush()).resolves.toBe(true);
		expect(file.body).toBe('a');
	});

	test('exclusive operations run after pending edits are saved', async () => {
		const { editor, session, file } = setup('a');
		editor.type('b', session);
		const seen = await session.exclusive(async () => file.body);
		expect(seen).toBe('ab');
	});
});
