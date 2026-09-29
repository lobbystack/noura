import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
	test as base,
	expect,
	type Locator,
	type Page,
} from '@playwright/test';

export { expect };

export const MERGED_MESSAGE = 'Merged changes from another app';

/**
 * How a note stores the text typed into it: line breaks around the text are
 * normalized and the file ends with one; everything else is kept as typed.
 */
export function noteFileBody(typed: string): string {
	const body = typed.replace(/^\n+/, '').replace(/[\r\n]+$/, '');
	return body.trim() ? `${body}\n` : '';
}

/** The built workspace worker, so tests can open a second one. */
const workerUrl = (() => {
	const directory = fileURLToPath(
		new URL('../../build/_app/immutable/workers/', import.meta.url),
	);
	let name: string | undefined;
	try {
		name = readdirSync(directory).find(
			(file) =>
				file.startsWith('browser-workspace.worker') && file.endsWith('.js'),
		);
	} catch {
		name = undefined;
	}
	return name ? `/_app/immutable/workers/${name}` : null;
})();

declare global {
	interface Window {
		__e2e?: {
			messages: string[];
			outside?: (command: string, payload?: object) => Promise<unknown>;
		};
	}
}

/**
 * Records every time a watched message appears, so a test can prove a
 * message that shows for two seconds never showed at all.
 */
function recordMessages(watched: string[]) {
	const state = { messages: [] as string[] };
	window.__e2e = state;
	const visible = new Set<string>();
	const check = () => {
		const text = document.body?.innerText ?? '';
		for (const message of watched) {
			const shown = text.includes(message);
			if (shown && !visible.has(message)) state.messages.push(message);
			if (shown) visible.add(message);
			else visible.delete(message);
		}
	};
	new MutationObserver(check).observe(document, {
		subtree: true,
		childList: true,
		characterData: true,
	});
}

export class App {
	constructor(readonly page: Page) {}

	/** The note or Markdown text editor. */
	editor(name = 'Note text'): Locator {
		return this.page.getByRole('textbox', { name, exact: true });
	}

	get fileName(): Locator {
		return this.page.getByRole('textbox', { name: 'File name' });
	}

	get tree(): Locator {
		return this.page.getByRole('tree', { name: 'Files' });
	}

	treeItem(name: string | RegExp): Locator {
		return this.tree.getByRole('treeitem', { name, exact: true });
	}

	async createWorkspace(name = 'E2E') {
		await this.page.goto('/');
		await this.page.getByLabel('New workspace').fill(name);
		await this.page.getByRole('button', { name: 'Create' }).click();
		await expect(
			this.page.getByRole('navigation', { name: 'Sections' }),
		).toBeVisible();
	}

	async openFiles() {
		await this.page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Files' })
			.click();
		await expect(
			this.page.getByRole('button', { name: 'New note' }),
		).toBeVisible();
	}

	/** Create a note with New note, name it, and put the caret in its text. */
	async newNote(name: string) {
		await this.openFiles();
		await this.page.getByRole('button', { name: 'New note' }).click();
		await expect(this.fileName).toBeFocused();
		await this.page.keyboard.press('ControlOrMeta+a');
		await this.page.keyboard.type(name);
		await this.page.keyboard.press('Enter');
		await expect(this.editor()).toBeFocused();
		await expect(this.treeItem(name)).toBeVisible();
	}

	/** The text the editor holds, which may differ from what it renders. */
	async doc(name = 'Note text'): Promise<string> {
		return this.editor(name).evaluate((element) => {
			const tile = (element as unknown as { cmTile?: EditorTile }).cmTile;
			const view = tile?.root?.view;
			if (!view) throw new Error('No editor view');
			return view.state.doc.toString();
		});
	}

	/** Caret offset in the editor's text. */
	async caret(name = 'Note text'): Promise<number> {
		return this.editor(name).evaluate((element) => {
			const tile = (element as unknown as { cmTile?: EditorTile }).cmTile;
			const view = tile?.root?.view;
			if (!view) throw new Error('No editor view');
			return view.state.selection.main.head;
		});
	}

	/** Put the caret at `offset` (or the end) with the keyboard. */
	async caretToEnd(name = 'Note text') {
		await this.editor(name).focus();
		await this.page.keyboard.press('ControlOrMeta+End');
	}

	/** Messages from `recordMessages` seen so far. */
	async messages(): Promise<string[]> {
		return this.page.evaluate(() => [...(window.__e2e?.messages ?? [])]);
	}

	async #outside<T>(command: string, payload: object = {}): Promise<T> {
		if (!workerUrl)
			throw new Error('Build the app first: the workspace worker is missing');
		return (await this.page.evaluate(
			async ({ url, command, payload }) => {
				const state = window.__e2e!;
				if (!state.outside) {
					const worker = new Worker(url, { type: 'module' });
					const pending = new Map<
						string,
						{ resolve(value: unknown): void; reject(error: Error): void }
					>();
					let next = 0;
					await new Promise<void>((resolve, reject) => {
						worker.addEventListener('message', (event) => {
							const data = event.data;
							if (data?.type === 'ready') resolve();
							else if (data?.type === 'startup-error')
								reject(new Error('The second worker did not start'));
							else if (data?.type === 'response') {
								const request = pending.get(data.id);
								pending.delete(data.id);
								if (data.ok) request?.resolve(data.value);
								else request?.reject(new Error(JSON.stringify(data.error)));
							}
						});
					});
					const request = (command: string, payload: object = {}) =>
						new Promise<unknown>((resolve, reject) => {
							const id = `outside-${next++}`;
							pending.set(id, { resolve, reject });
							worker.postMessage({ type: 'request', id, command, payload });
						});
					const id = localStorage.getItem('noura.browser.last-workspace');
					await request('workspace_open', {
						input: { path: `browser://${id}` },
					});
					state.outside = request;
				}
				return state.outside(command, payload);
			},
			{ url: workerUrl, command, payload },
		)) as T;
	}

	/**
	 * The bytes stored for `path`, read through a second workspace worker,
	 * as another tab would. Null when the file does not exist.
	 */
	async stored(path: string): Promise<string | null> {
		const file = await this.#outside<{ bytes: string } | null>(
			'storage_files_read',
			{ path },
		);
		if (!file) return null;
		return Buffer.from(file.bytes, 'base64').toString('utf8');
	}

	/**
	 * The body of a stored note: the bytes after its noura header and its
	 * `# Title` line, exactly as stored.
	 */
	async storedBody(path: string): Promise<string | null> {
		const text = await this.stored(path);
		if (text === null) return null;
		const match = /^---\n[\s\S]*?\n---\n\n?# [^\n]*\n\n?/.exec(text);
		if (!match) throw new Error(`${path} is not a note:\n${text}`);
		return text.slice(match[0].length);
	}

	/** Write `text` to `path` from outside this tab, like another app would. */
	async writeOutside(path: string, text: string | Uint8Array) {
		const current = await this.#outside<{ revision: string } | null>(
			'storage_files_read',
			{ path },
		);
		const bytes =
			typeof text === 'string' ? Buffer.from(text, 'utf8') : Buffer.from(text);
		await this.#outside('storage_files_write', {
			path,
			bytes: bytes.toString('base64'),
			expectedRevision: current?.revision ?? null,
		});
	}

	/**
	 * Hold the workspace storage lock, so saves stay in flight until
	 * `release` runs, like a slow disk.
	 */
	async holdStorage(): Promise<() => Promise<void>> {
		await this.page.evaluate(
			(name) =>
				new Promise<void>((held) => {
					void navigator.locks.request(
						name,
						{ mode: 'exclusive' },
						() =>
							new Promise<void>((release) => {
								(window as unknown as { __release?: () => void }).__release =
									release;
								held();
							}),
					);
				}),
			await this.#storageLock(),
		);
		return async () => {
			await this.page.evaluate(() =>
				(window as unknown as { __release?: () => void }).__release?.(),
			);
		};
	}

	/** True while a request is waiting for the storage lock. */
	async storageWaiting(): Promise<boolean> {
		return this.page.evaluate(
			async (name) => {
				const state = await navigator.locks.query();
				return (state.pending ?? []).some((lock) => lock.name === name);
			},
			await this.#storageLock(),
		);
	}

	/** The Web Lock the open workspace's storage operations take. */
	#storageLock(): Promise<string> {
		return this.page.evaluate(
			() =>
				`noura:browser-workspace:${localStorage.getItem('noura.browser.last-workspace')}`,
		);
	}

	async storedPaths(): Promise<string[]> {
		const entries =
			await this.#outside<Array<{ relativePath: string }>>('files_list');
		return entries.map((entry) => entry.relativePath).sort();
	}
}

type EditorTile = {
	root?: {
		view?: {
			state: {
				doc: { toString(): string };
				selection: { main: { head: number } };
			};
		};
	};
};

export const test = base.extend<{ app: App }>({
	app: async ({ page }, use) => {
		await page.addInitScript(recordMessages, [MERGED_MESSAGE]);
		const app = new App(page);
		await app.createWorkspace();
		await use(app);
	},
});
