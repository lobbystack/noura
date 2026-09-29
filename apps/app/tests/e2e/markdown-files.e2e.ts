import { expect, MERGED_MESSAGE, test, type App } from './fixtures';

const RICH = [
	'---',
	'title: Plain file',
	'tags: [one, two]',
	'---',
	'',
	'# Plain file',
	'',
	'Intro with trailing spaces  ',
	'',
	'| Name | Value |',
	'| ---- | ----: |',
	'| a    |     1 |',
	'',
	'```ts',
	'const a = 1;',
	'```',
	'',
	'> [!note] Callout title',
	'> Callout body',
	'',
	'- [ ] open task',
	'- [x] done task',
	'',
	'See [[Other note]] and [[Other note|alias]].',
	'',
	'Edit here',
	'',
].join('\n');

/** Open a plain Markdown file another app put in the workspace. */
async function openPlainFile(
	app: App,
	path: string,
	text: string | Uint8Array,
) {
	await app.openFiles();
	await app.writeOutside(path, text);
	const name = path.replace(/\.md$/, '');
	await app.treeItem(name).click();
	await expect(app.editor('File text')).toBeVisible();
}

/** Type `insert` right after `anchor`. */
async function typeAfter(app: App, anchor: string, insert: string) {
	await app.caretAfter(anchor, 'File text');
	await app.page.keyboard.type(insert);
}

test.describe('plain Markdown files', () => {
	test('editing keeps every other byte of the file', async ({ app, page }) => {
		await openPlainFile(app, 'Plain.md', RICH);
		await typeAfter(app, 'Edit here', ' and there ');
		const expected = RICH.replace('Edit here', 'Edit here and there ');
		await expect.poll(() => app.stored('Plain.md')).toBe(expected);
		await page.reload();
		await app.treeItem('Plain').click();
		await expect(app.editor('File text')).toBeVisible();
		expect(await app.doc('File text')).toBe(expected);
		expect(await app.stored('Plain.md')).toBe(expected);
		expect(await app.messages()).toEqual([]);
	});

	test('CRLF line endings and a byte order mark survive an edit', async ({
		app,
	}) => {
		const crlf = RICH.replaceAll('\n', '\r\n');
		const bytes = new Uint8Array([
			0xef,
			0xbb,
			0xbf,
			...new TextEncoder().encode(crlf),
		]);
		await openPlainFile(app, 'Windows.md', bytes);
		expect(await app.doc('File text')).toBe(RICH);
		await typeAfter(app, 'Edit here', '!');
		const expected = `﻿${crlf.replace('Edit here', 'Edit here!')}`;
		await expect.poll(() => app.stored('Windows.md')).toBe(expected);
		expect(await app.messages()).toEqual([]);
	});

	test('typing at the end of a file without a final newline', async ({
		app,
		page,
	}) => {
		await openPlainFile(app, 'Bare.md', 'no newline');
		await app.caretToEnd('File text');
		await page.keyboard.type(' ');
		await expect.poll(() => app.stored('Bare.md')).toBe('no newline ');
		await page.keyboard.press('Enter');
		await expect.poll(() => app.stored('Bare.md')).toBe('no newline \n');
		await page.keyboard.press('Enter');
		await page.keyboard.type('x');
		await expect.poll(() => app.stored('Bare.md')).toBe('no newline \n\nx');
		expect(await app.doc('File text')).toBe('no newline \n\nx');
		expect(await app.messages()).toEqual([]);
	});

	test('an outside edit made during typing is merged, shown, and kept', async ({
		app,
		page,
	}) => {
		await openPlainFile(app, 'Shared.md', 'first\n\nsecond\n\nthird\n');
		await typeAfter(app, 'first', ' local');
		await expect
			.poll(() => app.stored('Shared.md'))
			.toBe('first local\n\nsecond\n\nthird\n');
		// The outside edit lands before the debounced save of ' more' runs.
		await page.keyboard.type(' more');
		await app.writeOutside(
			'Shared.md',
			'first local\n\nsecond\n\nthird outside\n',
		);
		await expect
			.poll(() => app.stored('Shared.md'))
			.toBe('first local more\n\nsecond\n\nthird outside\n');
		await expect
			.poll(() => app.doc('File text'))
			.toBe('first local more\n\nsecond\n\nthird outside\n');
		// A later save keeps the outside edit.
		await page.keyboard.type('!');
		await expect
			.poll(() => app.stored('Shared.md'))
			.toBe('first local more!\n\nsecond\n\nthird outside\n');
		expect(await app.messages()).toEqual([MERGED_MESSAGE]);
	});
});

test.describe('notes written by another app', () => {
	test('editing a note keeps its other properties and text', async ({
		app,
		page,
	}) => {
		const header = [
			'---',
			'id: note_01j00000000000000000000001',
			'type: note',
			'aliases:',
			'- Plan',
			'rating: 4',
			'source: https://example.com/a?b=c',
			'tags:',
			'- work',
			'- q3',
			'created: 2026-09-01T10:00:00Z',
			'updated: 2026-09-01T10:00:00Z',
			'---',
			// Notes put a blank line after the header, as the format writes it.
			'',
			'',
		].join('\n');
		const body = RICH.slice(RICH.indexOf('# Plain file'))
			.replace('# Plain file', '# Imported')
			.replace(/\n$/, '');
		await app.openFiles();
		await app.writeOutside('Imported.md', `${header}${body}\n`);
		await app.treeItem('Imported').click();
		await expect(app.fileName).toHaveValue('Imported');
		const shown = await app.doc();
		await app.caretAfter('Edit here');
		await page.keyboard.type(' now');
		await expect
			.poll(() => app.stored('Imported.md'))
			.toContain('Edit here now');
		const stored = (await app.stored('Imported.md'))!;
		// Only the edit and the updated time differ.
		const expected = `${header}${body}\n`
			.replace('Edit here', 'Edit here now')
			.replace(/^updated: .*$/m, stored.match(/^updated: .*$/m)![0]);
		expect(stored).toBe(expected);
		expect(await app.doc()).toBe(shown.replace('Edit here', 'Edit here now'));
	});
});
