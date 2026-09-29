import { expect, noteFileBody, test, type App } from './fixtures';

async function writeNote(app: App, name: string, text: string) {
	await app.newNote(name);
	await app.page.keyboard.type(text);
	await expect
		.poll(() => app.storedBody(`${name}.md`))
		.toBe(noteFileBody(text));
}

function tab(app: App, name: string) {
	return app.page.getByRole('tab', { name, exact: true });
}

test.describe('document header', () => {
	test('renames the file from its title', async ({ app, page }) => {
		await writeNote(app, 'Old name', 'content stays');
		await app.fileName.click();
		await page.keyboard.press('ControlOrMeta+a');
		await page.keyboard.type('New name');
		await page.keyboard.press('Enter');
		await expect(app.editor()).toBeFocused();
		await expect(app.treeItem('New name')).toBeVisible();
		await expect(tab(app, 'New name')).toBeVisible();
		await expect.poll(() => app.storedPaths()).toEqual(['New name.md']);
		expect(await app.storedBody('New name.md')).toBe(
			noteFileBody('content stays'),
		);
		expect(await app.stored('New name.md')).toContain('\n# New name\n');
		// Typing right after the rename saves to the new file.
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type(' more');
		await expect
			.poll(() => app.storedBody('New name.md'))
			.toBe(noteFileBody('content stays more'));
	});

	test('a name that is taken or empty keeps the old name', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Taken', 'first');
		await writeNote(app, 'Mine', 'second');
		await app.fileName.click();
		await page.keyboard.press('ControlOrMeta+a');
		await page.keyboard.type('Taken');
		await page.keyboard.press('Enter');
		await expect(page.getByText(/already/i).first()).toBeVisible();
		await expect(app.fileName).toHaveValue('Mine');
		await app.fileName.click();
		await page.keyboard.press('ControlOrMeta+a');
		await page.keyboard.press('Backspace');
		await page.keyboard.press('Enter');
		await expect(app.fileName).toHaveValue('Mine');
		expect(await app.storedPaths()).toEqual(['Mine.md', 'Taken.md']);
		expect(await app.storedBody('Taken.md')).toBe(noteFileBody('first'));
		expect(await app.storedBody('Mine.md')).toBe(noteFileBody('second'));
	});
});

test.describe('tabs', () => {
	test('a click opens a preview tab that the next click replaces', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'One', 'one');
		await writeNote(app, 'Two', 'two');
		await writeNote(app, 'Three', 'three');
		// New notes open as kept tabs.
		await expect(page.getByRole('tab')).toHaveCount(3);
		await tab(app, 'One').click();
		await page.getByRole('button', { name: 'Close One' }).click();
		await page.getByRole('button', { name: 'Close Two' }).click();
		await expect(page.getByRole('tab')).toHaveText(['Three']);
		await app.treeItem('One').click();
		await expect(tab(app, 'One')).toHaveAttribute('title', 'One (preview)');
		await app.treeItem('Two').click();
		await expect(tab(app, 'One')).toBeHidden();
		await expect(tab(app, 'Two')).toHaveAttribute('title', 'Two (preview)');
		// Typing in a preview keeps it.
		await app.editor().focus();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type('!');
		await expect(tab(app, 'Two')).toHaveAttribute('title', 'Two');
		// A double-click in the tree opens a kept tab.
		await app.treeItem('One').dblclick();
		await expect(tab(app, 'One')).toHaveAttribute('title', 'One');
		await expect(page.getByRole('tab')).toHaveText(['Three', 'Two', 'One']);
		// Middle-click closes a tab.
		await tab(app, 'Three').click({ button: 'middle' });
		await expect(page.getByRole('tab')).toHaveText(['Two', 'One']);
		await expect(app.fileName).toHaveValue('One');
	});

	test('tabs come back after a reload', async ({ app, page }) => {
		await writeNote(app, 'Left', 'left');
		await writeNote(app, 'Right', 'right');
		await tab(app, 'Left').click();
		await expect(app.fileName).toHaveValue('Left');
		await page.reload();
		await expect(page.getByRole('tab')).toHaveText(['Left', 'Right']);
		await expect(page.getByRole('tab', { selected: true })).toHaveText('Left');
		await expect(app.fileName).toHaveValue('Left');
		expect(await app.doc()).toBe('left');
		await tab(app, 'Right').click();
		await expect(app.fileName).toHaveValue('Right');
		expect(await app.doc()).toBe('right');
	});

	test('switching tabs keeps each note’s text', async ({ app, page }) => {
		await writeNote(app, 'First', 'alpha');
		await writeNote(app, 'Second', 'beta');
		await page.keyboard.type(' typed');
		// Switch before the autosave delay passes.
		await tab(app, 'First').click();
		await expect(app.fileName).toHaveValue('First');
		expect(await app.doc()).toBe('alpha');
		await tab(app, 'Second').click();
		await expect(app.fileName).toHaveValue('Second');
		expect(await app.doc()).toBe('beta typed');
		await expect
			.poll(() => app.storedBody('Second.md'))
			.toBe(noteFileBody('beta typed'));
	});
});

test.describe('finding files', () => {
	test('quick open finds a note by name and Enter opens the best match', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Meeting notes', 'agenda');
		await writeNote(app, 'Groceries', 'milk');
		await writeNote(app, 'Meeting archive', 'old');
		await page.keyboard.press('ControlOrMeta+o');
		const palette = page.getByRole('dialog', { name: 'Search and commands' });
		await expect(palette).toBeVisible();
		await page.keyboard.type('groc');
		await expect(
			palette.getByRole('option', { name: /Groceries/ }),
		).toBeVisible();
		await page.keyboard.press('Enter');
		await expect(palette).toBeHidden();
		await expect(app.fileName).toHaveValue('Groceries');
		// Typing and pressing Enter at once opens the match, not "New note".
		await app.treeItem('Groceries').click();
		await page.keyboard.press('ControlOrMeta+o');
		await expect(palette).toBeVisible();
		await page.keyboard.type('meeting n');
		await page.keyboard.press('Enter');
		await expect(palette).toBeHidden();
		await expect(app.fileName).toHaveValue('Meeting notes');
		expect(await app.storedPaths()).toEqual([
			'Groceries.md',
			'Meeting archive.md',
			'Meeting notes.md',
		]);
	});

	test.fixme('quick open opens again right after opening a file', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Alpha', 'a');
		await writeNote(app, 'Beta', 'b');
		const palette = page.getByRole('dialog', { name: 'Search and commands' });
		for (const name of ['Alpha', 'Beta', 'Alpha']) {
			await page.keyboard.press('ControlOrMeta+o');
			await expect(palette).toBeVisible();
			await page.keyboard.type(name.toLowerCase());
			await expect(
				palette.getByRole('option', { name: new RegExp(name) }),
			).toBeVisible();
			await page.keyboard.press('Enter');
			await expect(app.fileName).toHaveValue(name);
		}
	});

	test.fixme('a shortcut pressed twice quickly runs twice', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'One', '1');
		await writeNote(app, 'Two', '2');
		await writeNote(app, 'Three', '3');
		await expect(page.getByRole('tab')).toHaveCount(3);
		await page.keyboard.press('ControlOrMeta+w');
		await page.keyboard.press('ControlOrMeta+w');
		await expect(page.getByRole('tab')).toHaveText(['One']);
		await expect(app.fileName).toHaveValue('One');
	});

	test('content search finds text inside notes', async ({ app, page }) => {
		await writeNote(app, 'Recipe', 'Add the saffron last');
		await writeNote(app, 'Other', 'nothing here');
		await page.keyboard.press('ControlOrMeta+Shift+f');
		const palette = page.getByRole('dialog', { name: 'Search and commands' });
		await expect(palette).toBeVisible();
		await page.keyboard.type('saffron');
		const result = palette.getByRole('option', { name: /Recipe/ });
		await expect(result).toBeVisible();
		await result.click();
		await expect(app.fileName).toHaveValue('Recipe');
	});
});

test.describe('wikilinks', () => {
	test('[[ completion inserts a link that opens the note', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Target page', 'destination');
		await app.newNote('Source');
		await page.keyboard.type('See [[Targ');
		const option = page.getByRole('option', { name: /Target page/ });
		await expect(option).toBeVisible();
		await page.keyboard.press('Enter');
		expect(await app.doc()).toBe('See [[Target page]]');
		await page.keyboard.type(' done');
		expect(await app.doc()).toBe('See [[Target page]] done');
		await expect
			.poll(() => app.storedBody('Source.md'))
			.toBe(noteFileBody('See [[Target page]] done'));
		// Move the caret away so the link renders, then follow it.
		await page.keyboard.press('ControlOrMeta+Home');
		await page.keyboard.press('Enter');
		await app.editor().getByText('Target page', { exact: true }).click();
		await expect(app.fileName).toHaveValue('Target page');
		expect(await app.doc()).toBe('destination');
	});

	test('Cmd-click opens a wikilink in a new tab', async ({ app, page }) => {
		await writeNote(app, 'Linked', 'linked text');
		await writeNote(app, 'Hub', 'Go to [[Linked]] now');
		await page.keyboard.press('ControlOrMeta+Home');
		await page.keyboard.press('Enter');
		await tab(app, 'Linked').click();
		await page.getByRole('button', { name: 'Close Linked' }).click();
		await expect(page.getByRole('tab')).toHaveText(['Hub']);
		await app
			.editor()
			.getByText('Linked', { exact: true })
			.click({ modifiers: ['ControlOrMeta'] });
		await expect(page.getByRole('tab')).toHaveText(['Hub', 'Linked']);
		await expect(tab(app, 'Linked')).toHaveAttribute('title', 'Linked');
	});

	test('a link to a missing note says so and changes nothing', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Start', 'Make [[Brand new]] here');
		await page.keyboard.press('ControlOrMeta+Home');
		await page.keyboard.press('Enter');
		await app.editor().getByText('Brand new', { exact: true }).click();
		await expect(
			page.getByText('“Brand new” isn’t in this workspace yet.'),
		).toBeVisible();
		await expect(app.fileName).toHaveValue('Start');
		expect(await app.storedPaths()).toEqual(['Start.md']);
	});
});

test.describe('coming back', () => {
	test('reopening the app shows the last page and its document', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Last', 'last text');
		await page.goto('/');
		await expect(page).toHaveURL(/\/files\?selected=/);
		await expect(app.fileName).toHaveValue('Last');
		expect(await app.doc()).toBe('last text');
	});

	test('the Files page shows the highlighted tab’s document', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Shown', 'shown text');
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		await expect(app.fileName).toBeHidden();
		await app.openFiles();
		await expect(page.getByRole('tab', { selected: true })).toHaveText('Shown');
		await expect(app.fileName).toHaveValue('Shown');
		expect(await app.doc()).toBe('shown text');
	});
});
