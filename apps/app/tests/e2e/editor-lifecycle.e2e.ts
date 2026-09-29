import { expect, noteFileBody, test, type App } from './fixtures';

async function expectStoredBody(app: App, path: string, typed: string) {
	await expect
		.poll(() => app.storedBody(path), { timeout: 10_000 })
		.toBe(noteFileBody(typed));
}

test.describe('leaving a note right after typing', () => {
	test('switching to another tab saves what was typed', async ({
		app,
		page,
	}) => {
		await app.newNote('Other');
		await app.newNote('Leaving');
		await page.keyboard.type('typed then left');
		await page.getByRole('tab', { name: 'Other', exact: true }).click();
		await expect(app.fileName).toHaveValue('Other');
		await expectStoredBody(app, 'Leaving.md', 'typed then left');
		await page.getByRole('tab', { name: 'Leaving', exact: true }).click();
		await expect(app.fileName).toHaveValue('Leaving');
		await expect
			.poll(() => app.doc(), { timeout: 5000 })
			.toBe('typed then left');
	});

	test('going Home while a save waits for the disk still saves', async ({
		app,
		page,
	}) => {
		await app.newNote('Slow leave');
		await page.keyboard.type('first ');
		await expectStoredBody(app, 'Slow leave.md', 'first ');
		const release = await app.holdStorage();
		await page.keyboard.type('second');
		await expect.poll(() => app.storageWaiting()).toBe(true);
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		await release();
		await expect(page.getByRole('heading', { level: 1 })).toContainText(/Good/);
		await expectStoredBody(app, 'Slow leave.md', 'first second');
	});

	test('closing the tab saves what was typed', async ({ app, page }) => {
		await app.newNote('Closing');
		await page.keyboard.type('before closing');
		await page.getByRole('button', { name: 'Close Closing' }).click();
		await expect(page.getByText('No file open')).toBeVisible();
		await expectStoredBody(app, 'Closing.md', 'before closing');
		// Cmd-W and a middle-click close tabs the same way.
		await app.newNote('Shortcut');
		await page.keyboard.type('closed with the keyboard');
		await page.keyboard.press('ControlOrMeta+w');
		await expect(page.getByRole('tab', { name: 'Shortcut' })).toBeHidden();
		await expectStoredBody(app, 'Shortcut.md', 'closed with the keyboard');
		await app.newNote('Middle');
		await page.keyboard.type('closed with the wheel');
		await page
			.getByRole('tab', { name: 'Middle', exact: true })
			.click({ button: 'middle' });
		await expect(page.getByRole('tab', { name: 'Middle' })).toBeHidden();
		await expectStoredBody(app, 'Middle.md', 'closed with the wheel');
	});

	test('renaming from the tree while typing keeps the typing', async ({
		app,
		page,
	}) => {
		await app.newNote('Before');
		await page.keyboard.type('some text');
		await app.treeItem('Before').click({ button: 'right' });
		await page.getByRole('menuitem', { name: 'Rename…' }).click();
		await page.keyboard.type('After');
		await page.keyboard.press('Enter');
		await expect(app.treeItem('After')).toBeVisible();
		await app.editor().focus();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type(' and more');
		await expectStoredBody(app, 'After.md', 'some text and more');
		expect(await app.storedPaths()).toEqual(['After.md']);
		expect(await app.doc()).toBe('some text and more');
	});

	test('undo after an outside edit merged keeps the outside edit', async ({
		app,
		page,
	}) => {
		await app.newNote('Undo merge');
		await page.keyboard.type('mine');
		await expectStoredBody(app, 'Undo merge.md', 'mine');
		const file = (await app.stored('Undo merge.md'))!;
		await app.writeOutside('Undo merge.md', `${file}\ntheirs\n`);
		await expect.poll(() => app.doc()).toBe('mine\n\ntheirs');
		await app.caretToEnd();
		await page.keyboard.type('!');
		await expectStoredBody(app, 'Undo merge.md', 'mine\n\ntheirs!');
		await page.keyboard.press('ControlOrMeta+z');
		expect(await app.doc()).toBe('mine\n\ntheirs');
		await expectStoredBody(app, 'Undo merge.md', 'mine\n\ntheirs');
	});
});

test.describe('the same note in two browser tabs', () => {
	test('typing in both keeps both', async ({ app, page, context }) => {
		await app.newNote('Shared');
		await page.keyboard.type('from one');
		await expectStoredBody(app, 'Shared.md', 'from one');
		const second = await context.newPage();
		await second.goto(page.url());
		const other = second.getByRole('textbox', {
			name: 'Note text',
			exact: true,
		});
		await expect(other).toBeVisible();
		await other.focus();
		await second.keyboard.press('ControlOrMeta+End');
		await second.keyboard.press('Enter');
		await second.keyboard.type('from two');
		await expectStoredBody(app, 'Shared.md', 'from one\nfrom two');
		await expect.poll(() => app.doc()).toBe('from one\nfrom two');
		await app.editor().focus();
		await page.keyboard.press('ControlOrMeta+Home');
		await page.keyboard.type('> ');
		await expectStoredBody(app, 'Shared.md', '> from one\nfrom two');
		await expect
			.poll(() =>
				other.evaluate((element) => {
					const tile = (
						element as unknown as {
							cmTile?: { root?: { view?: { state: { doc: object } } } };
						}
					).cmTile;
					return String(tile?.root?.view?.state.doc);
				}),
			)
			.toBe('> from one\nfrom two');
	});
});
