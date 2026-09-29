import { expect, noteFileBody, test, type App } from './fixtures';

async function expectStoredBody(app: App, path: string, typed: string) {
	await expect
		.poll(() => app.storedBody(path), { timeout: 10_000 })
		.toBe(noteFileBody(typed));
}

test.describe('editing a note', () => {
	test('keeps typing while a save waits for the disk', async ({
		app,
		page,
	}) => {
		await app.newNote('Slow');
		await page.keyboard.type('before ');
		await expectStoredBody(app, 'Slow.md', 'before ');
		const release = await app.holdStorage();
		await page.keyboard.type('during');
		// The debounced save starts and waits for the disk.
		await expect.poll(() => app.storageWaiting()).toBe(true);
		await page.keyboard.type(' more ');
		await page.keyboard.press('Enter');
		await page.keyboard.type('next');
		expect(await app.doc()).toBe('before during more \nnext');
		await release();
		await expectStoredBody(app, 'Slow.md', 'before during more \nnext');
		expect(await app.doc()).toBe('before during more \nnext');
		expect(await app.caret()).toBe('before during more \nnext'.length);
		expect(await app.messages()).toEqual([]);
	});

	test('Tab and Shift-Tab nest and lift list items', async ({ app, page }) => {
		await app.newNote('List');
		await page.keyboard.type('- one');
		await page.keyboard.press('Enter');
		await page.keyboard.type('two');
		await page.keyboard.press('Tab');
		expect(await app.doc()).toBe('- one\n  - two');
		await page.keyboard.press('Enter');
		await page.keyboard.type('three');
		await page.keyboard.press('Shift+Tab');
		expect(await app.doc()).toBe('- one\n  - two\n- three');
		await expectStoredBody(app, 'List.md', '- one\n  - two\n- three');
		// Tab outside a list keeps focus in the editor.
		await page.keyboard.press('Enter');
		await page.keyboard.press('Enter');
		await expect(app.editor()).toBeFocused();
		expect(await app.messages()).toEqual([]);
	});

	test('double spaces and a Markdown line break survive', async ({
		app,
		page,
	}) => {
		await app.newNote('Break');
		await page.keyboard.type('line  ');
		await page.keyboard.press('Enter');
		await page.keyboard.type('next  word');
		await expectStoredBody(app, 'Break.md', 'line  \nnext  word');
		expect(await app.doc()).toBe('line  \nnext  word');
	});

	test('text composed with an input method is kept', async ({ app, page }) => {
		await app.newNote('Compose');
		await page.keyboard.type('start ');
		const cdp = await page.context().newCDPSession(page);
		await cdp.send('Input.imeSetComposition', {
			text: 'に',
			selectionStart: 1,
			selectionEnd: 1,
		});
		await cdp.send('Input.imeSetComposition', {
			text: 'にほ',
			selectionStart: 2,
			selectionEnd: 2,
		});
		// Wait past the debounce while the composition is open.
		await page.waitForTimeout(700);
		await cdp.send('Input.insertText', { text: '日本' });
		await page.keyboard.type(' end');
		await expectStoredBody(app, 'Compose.md', 'start 日本 end');
		expect(await app.doc()).toBe('start 日本 end');
		expect(await app.messages()).toEqual([]);
	});

	test('formatting shortcuts wrap the selection', async ({ app, page }) => {
		await app.newNote('Format');
		const selectWord = async (word: string) => {
			await page.keyboard.type(word);
			for (let index = 0; index < word.length; index += 1)
				await page.keyboard.press('Shift+ArrowLeft');
		};
		await selectWord('bold');
		await page.keyboard.press('ControlOrMeta+b');
		await page.keyboard.press('End');
		await page.keyboard.type(' ');
		await selectWord('it');
		await page.keyboard.press('ControlOrMeta+i');
		await page.keyboard.press('End');
		await page.keyboard.type(' ');
		await selectWord('under');
		await page.keyboard.press('ControlOrMeta+u');
		await page.keyboard.press('End');
		await page.keyboard.type(' ');
		await selectWord('gone');
		await page.keyboard.press('ControlOrMeta+Shift+x');
		await page.keyboard.press('End');
		await page.keyboard.type(' ');
		await selectWord('site');
		// Cmd-K adds a link here; it must not also open the command palette.
		await page.keyboard.press('ControlOrMeta+k');
		await page.keyboard.type('https://example.com');
		await expect(app.editor()).toBeFocused();
		await expect(
			page.getByRole('dialog', { name: 'Search and commands' }),
		).toBeHidden();
		const expected =
			'**bold** *it* <u>under</u> ~~gone~~ [site](https://example.com)';
		expect(await app.doc()).toBe(expected);
		await expectStoredBody(app, 'Format.md', expected);
		// The same shortcut removes the formatting again.
		await app.editor().getByText('bold', { exact: true }).dblclick();
		await page.keyboard.press('ControlOrMeta+b');
		expect(await app.doc()).toBe(expected.replace('**bold**', 'bold'));
	});

	test('Cmd-K adds a link after the command palette has loaded', async ({
		app,
		page,
	}) => {
		const palette = page.getByRole('dialog', { name: 'Search and commands' });
		await page.getByRole('button', { name: 'Search' }).click();
		await expect(palette).toBeVisible();
		await page.keyboard.press('Escape');
		await expect(palette).toBeHidden();
		await app.newNote('Link');
		await page.keyboard.press('ControlOrMeta+k');
		await page.keyboard.type('label');
		expect(await app.doc()).toBe('[label]()');
		await expect(palette).toBeHidden();
		// Outside the editor, Cmd-K still opens the palette.
		await page.getByRole('button', { name: 'New folder' }).focus();
		await page.keyboard.press('ControlOrMeta+k');
		await expect(palette).toBeVisible();
	});

	test('toolbar buttons and checklists', async ({ app, page }) => {
		await app.newNote('Toolbar');
		await page.keyboard.type('buy milk');
		await page.getByRole('button', { name: 'Checklist' }).click();
		expect(await app.doc()).toBe('- [ ] buy milk');
		await app.caretToEnd();
		await page.keyboard.press('Enter');
		await page.keyboard.type('eggs');
		expect(await app.doc()).toBe('- [ ] buy milk\n- [ ] eggs');
		await page.keyboard.press('ControlOrMeta+Enter');
		expect(await app.doc()).toBe('- [ ] buy milk\n- [x] eggs');
		// Clicking the rendered checkbox checks the first item.
		await app.editor().locator('.cm-md-checkbox').first().click();
		expect(await app.doc()).toBe('- [x] buy milk\n- [x] eggs');
		await expectStoredBody(app, 'Toolbar.md', '- [x] buy milk\n- [x] eggs');
		await page.getByRole('button', { name: 'Undo', exact: true }).click();
		expect(await app.doc()).toBe('- [ ] buy milk\n- [x] eggs');
		await expectStoredBody(app, 'Toolbar.md', '- [ ] buy milk\n- [x] eggs');
		expect(await app.messages()).toEqual([]);
	});

	test('find and replace', async ({ app, page }) => {
		await app.newNote('Find');
		await page.keyboard.type('cat and cat and dog');
		await page.keyboard.press('ControlOrMeta+f');
		const find = page.getByRole('textbox', { name: /find/i }).first();
		await expect(find).toBeFocused();
		await page.keyboard.type('cat');
		const replace = page.getByRole('textbox', { name: /replace/i }).first();
		await replace.fill('bird');
		await page.getByRole('button', { name: /replace all/i }).click();
		expect(await app.doc()).toBe('bird and bird and dog');
		await expectStoredBody(app, 'Find.md', 'bird and bird and dog');
	});
});
