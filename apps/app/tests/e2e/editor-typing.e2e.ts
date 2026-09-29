import {
	expect,
	MERGED_MESSAGE,
	noteFileBody,
	test,
	type App,
} from './fixtures';

/** Wait until the note stored at `path` holds the text typed so far. */
async function expectStoredBody(app: App, path: string, typed: string) {
	await expect
		.poll(() => app.storedBody(path), { timeout: 10_000 })
		.toBe(noteFileBody(typed));
}

test.describe('typing in a note', () => {
	test('keeps spaces at the end of a line and of the note across saves', async ({
		app,
		page,
	}) => {
		await app.newNote('Spaces');
		const start = await app.doc();
		await page.keyboard.type('Hello ');
		// Let the debounced save run with the trailing space in place.
		await expectStoredBody(app, 'Spaces.md', `${start}Hello `);
		await page.keyboard.type(' world  ');
		await expectStoredBody(app, 'Spaces.md', `${start}Hello  world  `);
		expect(await app.doc()).toBe(`${start}Hello  world  `);
		expect(await app.caret()).toBe((await app.doc()).length);
		await page.keyboard.type('!');
		expect(await app.doc()).toBe(`${start}Hello  world  !`);
		await expectStoredBody(app, 'Spaces.md', `${start}Hello  world  !`);
		expect(await app.messages()).toEqual([]);
	});

	test('keeps Enter at the end, at the start of a line, and blank lines', async ({
		app,
		page,
	}) => {
		await app.newNote('Lines');
		const start = await app.doc();
		await page.keyboard.type('first');
		await page.keyboard.press('Enter');
		await expectStoredBody(app, 'Lines.md', `${start}first\n`);
		await page.keyboard.press('Enter');
		await expectStoredBody(app, 'Lines.md', `${start}first\n\n`);
		await page.keyboard.type('second');
		await expectStoredBody(app, 'Lines.md', `${start}first\n\nsecond`);
		// Enter at the start of a line pushes it down.
		await page.keyboard.press('Home');
		await page.keyboard.press('Enter');
		await expectStoredBody(app, 'Lines.md', `${start}first\n\n\nsecond`);
		expect(await app.doc()).toBe(`${start}first\n\n\nsecond`);
		await page.keyboard.type('x');
		expect(await app.doc()).toBe(`${start}first\n\n\nxsecond`);
		await expectStoredBody(app, 'Lines.md', `${start}first\n\n\nxsecond`);
		expect(await app.messages()).toEqual([]);
	});

	test('loses nothing when typing fast across the autosave delays', async ({
		app,
		page,
	}) => {
		await app.newNote('Fast');
		const start = await app.doc();
		let expected = start;
		// Type steadily for longer than the forced save (2 s), with short
		// pauses around the 300 ms debounce so saves start mid-typing.
		for (let round = 0; round < 12; round += 1) {
			const chunk = `word${round} `;
			await page.keyboard.type(chunk, { delay: 15 });
			expected += chunk;
			if (round % 3 === 2) {
				await page.keyboard.press('Enter');
				expected += '\n';
			}
			await page.waitForTimeout(round % 2 === 0 ? 320 : 90);
			expect(await app.doc()).toBe(expected);
			expect(await app.caret()).toBe(expected.length);
		}
		await expectStoredBody(app, 'Fast.md', expected);
		expect(await app.doc()).toBe(expected);
		expect(await app.messages()).toEqual([]);
	});

	test('undo and redo work after saves', async ({ app, page }) => {
		await app.newNote('Undo');
		const start = await app.doc();
		await page.keyboard.type('alpha ');
		await expectStoredBody(app, 'Undo.md', `${start}alpha `);
		await page.waitForTimeout(600);
		await page.keyboard.type('beta');
		await expectStoredBody(app, 'Undo.md', `${start}alpha beta`);
		await page.keyboard.press('ControlOrMeta+z');
		expect(await app.doc()).toBe(`${start}alpha `);
		await expectStoredBody(app, 'Undo.md', `${start}alpha `);
		await page.keyboard.press('ControlOrMeta+Shift+KeyZ');
		expect(await app.doc()).toBe(`${start}alpha beta`);
		await expectStoredBody(app, 'Undo.md', `${start}alpha beta`);
		expect(await app.messages()).toEqual([]);
	});

	test('reloading shows exactly what was typed', async ({ app, page }) => {
		await app.newNote('Reload');
		await page.keyboard.type('one  two ');
		await page.keyboard.press('Enter');
		await page.keyboard.press('Enter');
		await page.keyboard.type('  indented');
		await page.keyboard.press('Enter');
		await page.keyboard.type('last ');
		const typed = await app.doc();
		expect(typed).toBe('one  two \n\n  indented\n  last ');
		await expectStoredBody(app, 'Reload.md', typed);
		await page.reload();
		await app.treeItem('Reload').click();
		await expect(app.editor()).toBeVisible();
		expect(await app.doc()).toBe(typed);
		expect(await app.messages()).toEqual([]);
	});

	test('an outside edit merges with local typing and says so once', async ({
		app,
		page,
	}) => {
		await app.newNote('Outside');
		const start = await app.doc();
		await page.keyboard.type('local line');
		await expectStoredBody(app, 'Outside.md', `${start}local line`);
		const file = (await app.stored('Outside.md'))!;
		await app.writeOutside(
			'Outside.md',
			file.replace('local line\n', 'local line\n\nfrom outside\n'),
		);
		await expect.poll(() => app.doc()).toContain('from outside');
		// Keep typing: the text on screen and the file must keep both edits.
		await app.caretToEnd();
		await page.keyboard.type(' more');
		await expectStoredBody(
			app,
			'Outside.md',
			`${start}local line\n\nfrom outside more`,
		);
		expect(await app.doc()).toBe(`${start}local line\n\nfrom outside more`);

		// Typing before the outside edit is saved merges on the next save.
		await page.keyboard.press('ControlOrMeta+Home');
		const before = (await app.stored('Outside.md'))!;
		await page.keyboard.type('X');
		await app.writeOutside('Outside.md', `${before}tail\n`);
		await expectStoredBody(
			app,
			'Outside.md',
			`X${start}local line\n\nfrom outside more\ntail`,
		);
		await expect
			.poll(() => app.doc())
			.toBe(`X${start}local line\n\nfrom outside more\ntail`);
		expect(await app.messages()).toEqual([MERGED_MESSAGE]);
	});

	test('Enter in a new note’s name moves to its text at once', async ({
		app,
		page,
	}) => {
		// The text editor loads while the name is focused. On a slow machine
		// Enter comes first; the caret must still land in the text.
		await app.delayEditorLoad(1500);
		await app.openFiles();
		await page.getByRole('button', { name: 'New note' }).click();
		await expect(app.fileName).toBeFocused();
		await page.keyboard.type('Quick');
		await page.keyboard.press('Enter');
		await expect(app.editor()).toBeFocused();
		await page.keyboard.type('first words');
		expect(await app.doc()).toBe('first words');
		await expect.poll(() => app.storedBody('Quick.md')).toBe('first words\n');
	});
});
