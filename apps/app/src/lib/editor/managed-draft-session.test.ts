import { describe, expect, test } from 'bun:test';
import { rebaseText } from '@noura/editor/text-sync';
import type { ManagedDraftInput } from '@noura/workspace';
import {
	ManagedDraftSession,
	rebaseManagedDraft,
	type ManagedCanonical,
	type ManagedDraft,
	type ManagedDraftPort,
	type ManagedSaveResult,
} from './managed-draft-session';

/** A form like the task and project panes: the body rebases like CodeMirror. */
class FakeForm implements ManagedDraftPort {
	value: ManagedDraft;
	constructor(initial: ManagedDraft) {
		this.value = structuredClone(initial);
	}
	read() {
		return structuredClone(this.value);
	}
	rebase(from: ManagedDraft, to: ManagedDraft) {
		const fields = rebaseManagedDraft(this.value, from, to);
		this.value = {
			...fields,
			body: rebaseText(from.body, this.value.body, to.body),
		};
	}
	type(text: string, session: ManagedDraftSession<ManagedCanonical>) {
		this.value.body += text;
		session.edited();
	}
	set(
		key: string,
		value: unknown,
		session: ManagedDraftSession<ManagedCanonical>,
	) {
		this.value.properties = { ...this.value.properties, [key]: value };
		session.edited();
	}
}

/** A task file with a field-by-field three-way merge like the native side. */
class FakeFile {
	revision = 1;
	value: ManagedDraft;
	constructor(initial: ManagedDraft) {
		this.value = structuredClone(initial);
	}
	get canonical(): ManagedCanonical {
		return {
			id: 'task_1',
			revision: String(this.revision),
			...structuredClone(this.value),
		};
	}
	writeExternally(change: Partial<ManagedDraft>) {
		this.value = { ...this.value, ...structuredClone(change) };
		this.revision += 1;
	}
	save(input: ManagedDraftInput): ManagedSaveResult<ManagedCanonical> {
		const base: ManagedDraft = {
			title: input.baseTitle,
			body: input.baseBody,
			properties: input.baseProperties,
		};
		const local: ManagedDraft = {
			title: input.localTitle,
			body: input.localBody,
			properties: input.localProperties,
		};
		if (input.baseRevision === String(this.revision)) {
			this.writeExternally(local);
			return { status: 'unchanged', current: this.canonical };
		}
		const fields = rebaseManagedDraft(this.value, base, local);
		const merged = {
			...fields,
			body: rebaseText(base.body, local.body, this.value.body),
		};
		this.writeExternally(merged);
		return { status: 'merged', current: this.canonical, ...merged };
	}
}

function deferred() {
	let resolve!: () => void;
	const promise = new Promise<void>((done) => (resolve = done));
	return { promise, resolve };
}

function setup(initial: ManagedDraft) {
	const file = new FakeFile(initial);
	const form = new FakeForm(initial);
	let gate: Promise<void> | null = null;
	let readGate: Promise<void> | null = null;
	const session = new ManagedDraftSession<ManagedCanonical>({
		base: file.canonical,
		// Timers never fire on their own; tests flush explicitly.
		autosave: { schedule: () => () => {} },
		save: async (input) => {
			if (gate) await gate;
			return file.save(input);
		},
		read: async () => {
			const snapshot = file.canonical;
			if (readGate) await readGate;
			return snapshot;
		},
		onConflict: () => {
			throw new Error('unexpected conflict');
		},
	});
	session.attach(form);
	const hold = (set: (value: Promise<void> | null) => void) => {
		const waiting = deferred();
		set(waiting.promise);
		return () => {
			set(null);
			waiting.resolve();
		};
	};
	return {
		file,
		form,
		session,
		holdSaves: () => hold((value) => (gate = value)),
		holdReads: () => hold((value) => (readGate = value)),
	};
}

const task: ManagedDraft = {
	title: 'Plan',
	body: 'line\n',
	properties: { status: 'todo' },
};

describe('ManagedDraftSession', () => {
	test('text typed during a save stays on screen and is saved next', async () => {
		const { form, session, file, holdSaves } = setup(task);
		form.type('one', session);
		const release = holdSaves();
		const flushing = session.flush();
		await Promise.resolve();
		form.type(' two', session);
		release();
		await flushing;
		expect(form.value.body).toBe('line\none two');
		await session.flush();
		expect(file.value.body).toBe('line\none two');
	});

	test('a merge during a save cannot revert the external edit later', async () => {
		const { form, session, file, holdSaves } = setup(task);
		form.type('one', session);
		const release = holdSaves();
		const flushing = session.flush();
		await Promise.resolve();
		// Another app edits the file while the save waits, and the user
		// keeps typing and changes the status.
		file.writeExternally({ body: 'outside\nline\n', title: 'Plan it' });
		form.type(' two', session);
		form.set('status', 'done', session);
		release();
		await flushing;
		expect(form.value).toEqual({
			title: 'Plan it',
			body: 'outside\nline\none two',
			properties: { status: 'done' },
		});
		await session.flush();
		expect(file.value).toEqual(form.value);
	});

	test('a reload keeps edits made while the file was being read', async () => {
		const { form, session, file, holdReads } = setup(task);
		file.writeExternally({
			body: 'line\nfrom outside\n',
			properties: { status: 'todo', priority: 'high' },
		});
		const release = holdReads();
		const reloading = session.externalChange();
		await Promise.resolve();
		form.type('b', session);
		form.set('status', 'in-progress', session);
		release();
		await reloading;
		expect(form.value).toEqual({
			title: 'Plan',
			body: 'line\nfrom outside\nb',
			properties: { status: 'in-progress', priority: 'high' },
		});
		await session.flush();
		expect(file.value).toEqual(form.value);
	});

	test('an external change with unsaved edits saves them merged', async () => {
		const { form, session, file } = setup(task);
		form.type('mine', session);
		file.writeExternally({ body: 'theirs\nline\n' });
		await session.externalChange();
		expect(file.value.body).toBe('theirs\nline\nmine');
		expect(form.value.body).toBe('theirs\nline\nmine');
	});

	test('a clean reload follows the file', async () => {
		const { form, session, file } = setup(task);
		file.writeExternally({ title: 'Renamed', properties: {} });
		await session.externalChange();
		expect(form.value).toEqual({
			title: 'Renamed',
			body: 'line\n',
			properties: {},
		});
		expect(session.hasPendingWork).toBe(false);
		expect(session.base.revision).toBe(file.canonical.revision);
	});

	test('a resolved conflict shown on screen keeps no pending work', () => {
		const { form, session, file } = setup(task);
		form.type('draft', session);
		file.writeExternally({ body: 'resolved\n' });
		session.resolved(file.canonical, true);
		expect(form.value.body).toBe('resolved\n');
		expect(session.hasPendingWork).toBe(false);
	});
});

describe('rebaseManagedDraft', () => {
	test('keeps fields changed on screen and takes the rest from the target', () => {
		expect(
			rebaseManagedDraft(
				{ title: 'Mine', body: 'x', properties: { a: 1, b: 2, c: 3 } },
				{ title: 'Old', body: 'x', properties: { a: 1, b: 1, c: 3 } },
				{ title: 'Theirs', body: 'y', properties: { a: 9, b: 1 } },
			),
		).toEqual({ title: 'Mine', body: 'x', properties: { a: 9, b: 2 } });
	});
});
