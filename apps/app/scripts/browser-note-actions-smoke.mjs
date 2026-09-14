// Run against the browser app with Bun. PLAYWRIGHT_MODULE may point to an
// externally installed playwright module; no project dependency is required.
// Always uses a fresh non-persistent context, never the user's browser profile.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const { chromium } = await import(
	process.env.PLAYWRIGHT_MODULE || 'playwright'
);
const browser = await chromium.launch({ channel: 'chrome', headless: true });
try {
	const context = await browser.newContext({ acceptDownloads: true });
	const page = await context.newPage();
	const errors = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const button = (name) => page.getByRole('button', { name, exact: true });
	const field = (name) => page.getByLabel(name, { exact: true });
	const status = (text) =>
		page.getByRole('status').filter({ hasText: text }).waitFor();
	const body = 'Move preserves Unicode: 日本語 🦉\n\nDisposable smoke note.';
	await page.goto(
		`${process.env.NOURA_SMOKE_URL || 'http://127.0.0.1:4187'}/notes`,
	);
	await field('New workspace name').fill('Disposable note actions smoke');
	await button('Create browser workspace').click();
	await button('New note').click();
	await field('Title').fill('Move smoke');
	await field('Markdown').fill(body);
	await button('Save note').click();
	await status('Saved to browser storage.');
	await field('Markdown').fill(body + '\nUnsaved draft');
	assert(await button('Move note').isDisabled());
	assert(await button('Move to trash').isDisabled());
	assert(await field('Destination path').isDisabled());
	// A cancelled discard must not lose the draft.
	page.once('dialog', (dialog) => dialog.dismiss());
	await button('Discard draft / reload saved note').click();
	assert.equal(await field('Markdown').inputValue(), body + '\nUnsaved draft');
	await button('Save note').click();
	await status('Saved to browser storage.');
	const savedBody = body + '\nUnsaved draft';
	const originalPath = await field('Destination path').inputValue();
	await field('Destination path').fill('../escape.md');
	await button('Move note').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Markdown').inputValue(), savedBody);
	assert.equal(await field('Destination path').inputValue(), '../escape.md');
	// Successful move retains selection and supplies the next expected revision.
	const movedPath = 'Notes/Archive/moved-smoke.md';
	await field('Destination path').fill(movedPath);
	await button('Move note').click();
	await status('Note moved. Content preserved.');
	assert.equal(await field('Markdown').inputValue(), savedBody);
	await page.reload();
	await button('Open Disposable note actions smoke').click();
	await button('Move smoke').click();
	assert.equal(await field('Destination path').inputValue(), movedPath);
	assert.equal(await field('Markdown').inputValue(), savedBody);
	await button('New note').click();
	await field('Title').fill('Occupied destination');
	await field('Markdown').fill('Do not overwrite this note.');
	await button('Save note').click();
	await status('Saved to browser storage.');
	const occupiedPath = await field('Destination path').inputValue();
	await button('Move smoke').click();
	await field('Destination path').fill(occupiedPath);
	await button('Move note').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Markdown').inputValue(), savedBody);
	await button('Occupied destination').click();
	await page.waitForFunction(
		() =>
			document.querySelector('#note-body').value ===
			'Do not overwrite this note.',
	);
	assert.equal(
		await field('Markdown').inputValue(),
		'Do not overwrite this note.',
	);
	await button('Move smoke').click();
	// Modify only this disposable context's canonical file to provoke stale revisions.
	await page.waitForFunction(
		(expected) => document.querySelector('#note-body').value === expected,
		savedBody,
	);
	await page.evaluate(async (relativePath) => {
		async function locate(dir) {
			try {
				const metadata = await dir.getDirectoryHandle('.noura');
				await metadata.getFileHandle('workspace.yaml');
				return dir;
			} catch {
				/* Search nested OPFS storage directories. */
			}
			for await (const [, handle] of dir.entries()) {
				if (handle.kind === 'directory') {
					const found = await locate(handle);
					if (found) return found;
				}
			}
		}
		let dir = await locate(await navigator.storage.getDirectory());
		if (!dir) throw new Error('Disposable workspace not found');
		const parts = relativePath.split('/');
		const filename = parts.pop();
		for (const part of parts) dir = await dir.getDirectoryHandle(part);
		const file = await dir.getFileHandle(filename);
		const text = await (await file.getFile()).text();
		const writer = await file.createWritable();
		await writer.write(text + '\nExternal edit');
		await writer.close();
	}, movedPath);
	await field('Markdown').fill(savedBody + '\nKeep this failed-save draft');
	await button('Save note').click();
	await page.getByRole('alert').waitFor();
	assert.equal(
		await field('Markdown').inputValue(),
		savedBody + '\nKeep this failed-save draft',
	);
	assert(await button('Move note').isDisabled());
	assert(await button('Move to trash').isDisabled());
	// Restore the editor text without refreshing its revision, to test both mutations.
	await field('Markdown').fill(savedBody);
	await field('Destination path').fill('Notes/stale.md');
	await button('Move note').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Markdown').inputValue(), savedBody);
	page.once('dialog', (dialog) => dialog.accept());
	await button('Move to trash').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Markdown').inputValue(), savedBody);
	assert.equal(await button('Move smoke').count(), 1);
	await button('Discard draft / reload saved note').click();
	await field('Markdown').filter({ visible: true }).waitFor();
	await page.waitForFunction(() =>
		document.querySelector('#note-body').value.includes('External edit'),
	);
	page.once('dialog', (dialog) => dialog.dismiss());
	await button('Move to trash').click();
	await status('Move to trash cancelled');
	assert.equal(await button('Move smoke').count(), 1);
	const beforeDownload = page.waitForEvent('download');
	await button('Download JSON backup').click();
	const before = JSON.parse(
		await readFile(await (await beforeDownload).path(), 'utf8'),
	);
	const liveBytes = before.entries.find(
		(entry) => entry.path === movedPath,
	).base64;
	page.once('dialog', async (dialog) => {
		assert.match(dialog.message(), /UI is not yet available/);
		await dialog.accept();
	});
	await button('Move to trash').click();
	await status('Note moved to trash.');
	assert.equal(await button('Move smoke').count(), 0);
	assert.equal(await field('Markdown').count(), 0);
	await page.reload();
	await button('Open Disposable note actions smoke').click();
	await button('Occupied destination').waitFor();
	assert.equal(await button('Move smoke').count(), 0);
	const downloaded = page.waitForEvent('download');
	await button('Download JSON backup').click();
	const backup = JSON.parse(
		await readFile(await (await downloaded).path(), 'utf8'),
	);
	assert(
		!backup.entries.some((entry) =>
			[originalPath, movedPath, 'Notes/stale.md'].includes(entry.path),
		),
	);
	const trash = backup.entries.filter((entry) =>
		entry.path.startsWith('.noura/trash/'),
	);
	assert(trash.some((entry) => entry.base64 === liveBytes));
	assert(Buffer.from(liveBytes, 'base64').toString().includes(savedBody));
	assert(Buffer.from(liveBytes, 'base64').toString().includes('External edit'));
	assert.deepEqual(errors, []);
	console.log(
		'PASS: dirty guards, cancelled discard, invalid move, move/reload content, stale move/delete failures preserve editor, trash cancellation, delete/reload, backup retains trash; no page errors.',
	);
} finally {
	await browser.close();
}
