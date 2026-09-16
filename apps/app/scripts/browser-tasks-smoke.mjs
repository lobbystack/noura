// Bun + Playwright; always disposable contexts, never a user's browser profile.
// Start the app first. PLAYWRIGHT_MODULE can point to an external installation.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const { chromium } = await import(
	process.env.PLAYWRIGHT_MODULE || 'playwright'
);
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const url = process.env.NOURA_SMOKE_URL || 'http://127.0.0.1:4187';
try {
	const context = await browser.newContext({ acceptDownloads: true });
	const page = await context.newPage();
	const errors = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const button = (name) => page.getByRole('button', { name, exact: true });
	const field = (name) => page.getByLabel(name, { exact: true });
	const status = async (text) => {
		try {
			await page.getByRole('status').filter({ hasText: text }).waitFor();
		} catch (cause) {
			throw new Error(`${text}: ${await page.locator('main').innerText()}`, {
				cause,
			});
		}
	};
	const choose = async (name, value) => {
		await field(name).click();
		await page.getByRole('option', { name: value, exact: true }).click();
	};
	await page.goto(`${url}/tasks`);
	await field('New workspace name').fill('Disposable tasks smoke');
	await button('Create browser workspace').click();
	await button('New task').click();
	await field('Title').fill('Task smoke');
	await field('Markdown').fill('Original body 日本語 🦉');
	await choose('Status', 'in-progress');
	await choose('Priority', 'high');
	await field('Due').fill('2026-12-01T10:00:00+02:00');
	await button('Save task').click();
	await status('Saved to browser storage.');
	await field('Title').fill('Updated task');
	await field('Markdown').fill('Updated body 日本語 🦉');
	assert(await button('Complete task').isDisabled());
	assert(await button('Move task').isDisabled());
	assert(await button('Move to trash').isDisabled());
	assert(await button('Download JSON backup').isDisabled());
	page.once('dialog', (dialog) => dialog.dismiss());
	await page.getByRole('link', { name: 'Notes', exact: true }).click();
	assert.equal(await field('Title').inputValue(), 'Updated task');
	await button('Rebuild / refresh tasks').click();
	await status('Draft unchanged.');
	assert.equal(await field('Title').inputValue(), 'Updated task');
	await field('Due').fill('tomorrow');
	await button('Save task').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Due').inputValue(), 'tomorrow');
	assert.equal(await field('Markdown').inputValue(), 'Updated body 日本語 🦉');
	await field('Due').fill('2026-12-02');
	await button('Save task').click();
	await status('Saved to browser storage.');
	await button('Complete task').click();
	await status('Task status saved');
	await button('Reopen task').waitFor();
	await choose('Filter task status', 'todo');
	assert.equal(await button('Updated task · done').count(), 0);
	await choose('Filter task status', 'done');
	await button('Updated task · done').waitFor();
	await field('Filter task titles').fill('absent');
	assert.equal(await button('Updated task · done').count(), 0);
	await field('Filter task titles').fill('');
	await button('Reopen task').click();
	await status('Task status saved');
	await choose('Filter task status', 'all');
	await field('Due').fill('');
	await button('Save task').click();
	await status('Saved to browser storage.');
	await field('Destination path').fill('../escape.md');
	await button('Move task').click();
	await page.getByRole('alert').waitFor();
	await field('Destination path').fill('Tasks/Archive/task.md');
	await button('Move task').click();
	await status('Task moved. Content preserved.');
	await page.reload();
	await button('Open Disposable tasks smoke').click();
	await button('Updated task · todo').click();
	await button('Complete task').waitFor();
	assert.equal(await field('Markdown').inputValue(), 'Updated body 日本語 🦉');
	assert.equal(await field('Due').inputValue(), '');
	assert.equal(
		await field('Destination path').inputValue(),
		'Tasks/Archive/task.md',
	);
	assert.equal((await field('Priority').textContent()).trim(), 'high');
	// Edit only the disposable context's canonical task file behind the worker.
	await page.evaluate(async () => {
		async function locate(dir) {
			try {
				const metadata = await dir.getDirectoryHandle('.noura');
				await metadata.getFileHandle('workspace.yaml');
				return dir;
			} catch {
				/* Continue through OPFS storage containers. */
			}
			for await (const [, handle] of dir.entries()) {
				if (handle.kind === 'directory') {
					const found = await locate(handle);
					if (found) return found;
				}
			}
		}
		let dir = await locate(await navigator.storage.getDirectory());
		for (const part of ['Tasks', 'Archive'])
			dir = await dir.getDirectoryHandle(part);
		const handle = await dir.getFileHandle('task.md');
		const text = await (await handle.getFile()).text();
		const writer = await handle.createWritable();
		await writer.write(
			text.replace('\n---\n', '\ncustom_smoke: retained\n---\n'),
		);
		await writer.close();
	});
	await button('Complete task').click();
	await page.getByRole('alert').waitFor();
	await field('Title').fill('Keep stale draft');
	await button('Rebuild / refresh tasks').click();
	await status('Draft unchanged.');
	await button('Save task').click();
	await page.getByRole('alert').waitFor();
	assert.equal(await field('Title').inputValue(), 'Keep stale draft');
	page.once('dialog', (dialog) => dialog.accept());
	await button('Discard draft / reload saved task').click();
	await page.waitForFunction(
		() => document.querySelector('#note-title').value === 'Updated task',
	);
	await button('Complete task').click();
	await status('Task status saved');
	const download = page.waitForEvent('download');
	await button('Download JSON backup').click();
	const backupPath = await (await download).path();
	const backup = JSON.parse(await readFile(backupPath, 'utf8'));
	const taskFile = backup.entries.find(
		(entry) => entry.path === 'Tasks/Archive/task.md',
	);
	assert(taskFile);
	assert.match(
		Buffer.from(taskFile.base64, 'base64').toString(),
		/status: done/,
	);
	assert.match(
		Buffer.from(taskFile.base64, 'base64').toString(),
		/custom_smoke: retained/,
	);
	// Restore in a second empty browser context to retain the exact workspace identity.
	const restoredContext = await browser.newContext();
	const restored = await restoredContext.newPage();
	restored.on('pageerror', (error) => errors.push(error.message));
	await restored.goto(`${url}/tasks`);
	await restored.getByLabel('Import Noura JSON backup').waitFor();
	restored.once('dialog', (dialog) => dialog.accept());
	await restored
		.getByLabel('Import Noura JSON backup')
		.setInputFiles(backupPath);
	await restored
		.getByRole('button', { name: 'Updated task · done', exact: true })
		.click();
	await restored
		.getByRole('button', { name: 'Reopen task', exact: true })
		.waitFor();
	assert.equal(
		await restored.getByLabel('Markdown', { exact: true }).inputValue(),
		'Updated body 日本語 🦉',
	);
	assert.equal(
		await restored.getByLabel('Destination path').inputValue(),
		'Tasks/Archive/task.md',
	);
	page.once('dialog', (dialog) => dialog.dismiss());
	await button('Move to trash').click();
	await status('Move to trash cancelled');
	page.once('dialog', (dialog) => dialog.accept());
	await button('Move to trash').click();
	await status('Task moved to trash.');
	await page.reload();
	await button('Open Disposable tasks smoke').click();
	await button('New task').waitFor();
	assert.equal(await button('Updated task · done').count(), 0);
	assert.deepEqual(errors, []);
	console.log(
		'PASS: task create/edit, status/priority/date, filters, dirty guards, validation errors, complete/reopen, move, reload, backup/restore, trash; no page errors.',
	);
} finally {
	await browser.close();
}
