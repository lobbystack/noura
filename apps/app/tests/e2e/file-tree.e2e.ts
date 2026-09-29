import { expect, noteFileBody, test, type App } from './fixtures';

/**
 * Create a note or folder from the tree's context menu: on `parent`, or on
 * the blank space below the rows for the workspace root.
 */
async function create(
	app: App,
	kind: 'note' | 'folder',
	name: string,
	parent?: string,
) {
	const page = app.page;
	await app.openFiles();
	if (parent) {
		await app.treeItem(parent).click({ button: 'right' });
	} else {
		const rows = page.getByRole('tree', { name: 'Files' });
		const area = (
			(await rows.count()) > 0 ? rows : page.getByText('This folder is empty.')
		).locator('..');
		const box = (await area.boundingBox())!;
		await area.click({
			button: 'right',
			position: { x: 30, y: box.height - 6 },
		});
	}
	await page
		.getByRole('menuitem', {
			name: kind === 'note' ? 'New note' : 'New folder',
		})
		.click();
	const input =
		kind === 'note'
			? app.fileName
			: page.getByRole('textbox', { name: /^New name for / });
	await expect(input).toBeFocused();
	await page.keyboard.press('ControlOrMeta+a');
	await page.keyboard.type(name);
	await page.keyboard.press('Enter');
	await expect(app.treeItem(name)).toBeVisible();
	if (kind === 'note') await expect(app.editor()).toBeFocused();
}

/** A note at the workspace root holding `text`. */
async function writeNote(app: App, name: string, text: string) {
	await create(app, 'note', name);
	await app.page.keyboard.type(text);
	await expect
		.poll(() => app.storedBody(`${name}.md`))
		.toBe(noteFileBody(text));
}

test.describe('file tree', () => {
	test('creates notes and folders at the root and inside a folder', async ({
		app,
		page,
	}) => {
		await create(app, 'folder', 'Projects');
		await create(app, 'note', 'Inside', 'Projects');
		await create(app, 'folder', 'Nested', 'Projects');
		await create(app, 'note', 'Deep', 'Nested');
		await create(app, 'note', 'Root note');
		await create(app, 'folder', 'Top');
		// The New note button uses the folder of the selected row.
		await app.treeItem('Inside').click();
		await page.getByRole('button', { name: 'New note' }).click();
		await expect(app.fileName).toBeFocused();
		await page.keyboard.type('Sibling');
		await page.keyboard.press('Enter');
		await expect
			.poll(() => app.storedPaths())
			.toEqual([
				'Projects',
				'Projects/Inside.md',
				'Projects/Nested',
				'Projects/Nested/Deep.md',
				'Projects/Sibling.md',
				'Root note.md',
				'Top',
			]);
	});

	test('F2 renames in the tree, including a change of case only', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'draft', 'body text');
		await app.treeItem('draft').focus();
		await page.keyboard.press('F2');
		const input = page.getByRole('textbox', { name: 'New name for draft' });
		await expect(input).toBeFocused();
		await page.keyboard.type('Final');
		await page.keyboard.press('Enter');
		await expect(app.treeItem('Final')).toBeVisible();
		await expect(app.fileName).toHaveValue('Final');
		await app.treeItem('Final').focus();
		await page.keyboard.press('F2');
		await page.keyboard.type('FINAL');
		await page.keyboard.press('Enter');
		await expect(app.treeItem('FINAL')).toBeVisible();
		await expect(app.fileName).toHaveValue('FINAL');
		await expect.poll(() => app.storedPaths()).toContain('FINAL.md');
		expect(await app.storedPaths()).not.toContain('Final.md');
		expect(await app.storedBody('FINAL.md')).toBe(noteFileBody('body text'));
		expect(await app.doc()).toBe('body text');
	});

	test('F2 renames the open file after clicking it in the tree', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Clicked', 'keep this text');
		await app.newNote('Other');
		await app.treeItem('Clicked').click();
		await expect(app.fileName).toHaveValue('Clicked');
		await page.keyboard.press('F2');
		await expect(app.fileName).toBeFocused();
		// The whole name is selected, so typing replaces it.
		await page.keyboard.type('Renamed');
		await page.keyboard.press('Enter');
		await expect(app.treeItem('Renamed')).toBeVisible();
		await expect.poll(() => app.storedPaths()).toContain('Renamed.md');
		expect(await app.storedBody('Renamed.md')).toBe(
			noteFileBody('keep this text'),
		);
		expect(await app.doc()).toBe('keep this text');
	});

	test('drag and drop moves a note into a folder and keeps it open', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Moving', 'moved text');
		await create(app, 'folder', 'Archive');
		await app.treeItem('Moving').click();
		await app.treeItem('Moving').dragTo(app.treeItem('Archive'));
		await expect.poll(() => app.storedPaths()).toContain('Archive/Moving.md');
		await expect(app.treeItem('Moving')).toHaveAttribute('aria-level', '2');
		await page.keyboard.press('ControlOrMeta+End');
		expect(await app.doc()).toBe('moved text');
		await app.editor().focus();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type(' more');
		await expect
			.poll(() => app.storedBody('Archive/Moving.md'))
			.toBe(noteFileBody('moved text more'));
		expect(await app.storedPaths()).not.toContain('Moving.md');
	});

	test('duplicate copies a note', async ({ app, page }) => {
		await writeNote(app, 'Original', 'copy me');
		await app.treeItem('Original').click({ button: 'right' });
		await page.getByRole('menuitem', { name: 'Duplicate' }).click();
		await expect.poll(() => app.storedPaths()).toHaveLength(2);
		const copy = (await app.storedPaths()).find(
			(path) => path !== 'Original.md',
		)!;
		expect(await app.storedBody(copy)).toBe(noteFileBody('copy me'));
		const original = await app.stored('Original.md');
		const duplicate = await app.stored(copy);
		// The copy is a new object with its own ID.
		expect(duplicate!.match(/^id: (.*)$/m)?.[1]).not.toBe(
			original!.match(/^id: (.*)$/m)?.[1],
		);
	});

	test('trashing the open note closes its editor and its tab', async ({
		app,
		page,
	}) => {
		await writeNote(app, 'Keep', 'kept');
		await writeNote(app, 'Doomed', 'unsaved soon');
		await page.keyboard.type(' last words');
		await app.treeItem('Doomed').click({ button: 'right' });
		await page.getByRole('menuitem', { name: 'Move to Trash' }).click();
		await expect(app.treeItem('Doomed')).toBeHidden();
		await expect(
			page.getByRole('tab', { name: 'Doomed', exact: true }),
		).toBeHidden();
		// The tab that takes its place is the document on screen.
		const selected = page.getByRole('tab', { selected: true });
		await expect(selected).toHaveText('Keep');
		await expect(app.fileName).toHaveValue('Keep');
		expect(await app.doc()).toBe('kept');
		await expect.poll(() => app.storedPaths()).toEqual(['Keep.md']);

		// Trashing the last open document leaves nothing open or highlighted.
		await app.treeItem('Keep').click({ button: 'right' });
		await page.getByRole('menuitem', { name: 'Move to Trash' }).click();
		await expect(page.getByText('No file open')).toBeVisible();
		await expect(page.getByRole('tab', { selected: true })).toHaveCount(0);
		await expect(app.editor()).toBeHidden();
	});

	test('keyboard navigation opens files and folders', async ({ app, page }) => {
		await writeNote(app, 'Beta', 'beta text');
		await writeNote(app, 'Gamma', 'gamma text');
		await create(app, 'folder', 'Alpha');
		await app.treeItem('Alpha').focus();
		await page.keyboard.press('ArrowDown');
		await expect(app.treeItem('Beta')).toBeFocused();
		await page.keyboard.press('Enter');
		await expect(app.fileName).toHaveValue('Beta');
		await app.treeItem('Beta').focus();
		await page.keyboard.press('End');
		await expect(app.treeItem('Gamma')).toBeFocused();
		await page.keyboard.press('Home');
		await expect(app.treeItem('Alpha')).toBeFocused();
		await page.keyboard.press('g');
		await expect(app.treeItem('Gamma')).toBeFocused();
		await page.keyboard.press(' ');
		await expect(app.fileName).toHaveValue('Gamma');
	});

	test('expanded folders stay expanded after a reload', async ({
		app,
		page,
	}) => {
		await create(app, 'folder', 'Open');
		await create(app, 'folder', 'Closed');
		await create(app, 'note', 'Child', 'Open');
		await expect(app.treeItem('Open')).toHaveAttribute('aria-expanded', 'true');
		await expect(app.treeItem('Closed')).toHaveAttribute(
			'aria-expanded',
			'false',
		);
		await page.goto('/files');
		await expect(app.treeItem('Open')).toHaveAttribute('aria-expanded', 'true');
		await expect(app.treeItem('Child')).toBeVisible();
		await expect(app.treeItem('Closed')).toHaveAttribute(
			'aria-expanded',
			'false',
		);
	});
});
