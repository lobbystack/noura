// Disposable real Chromium contexts only. Start the app before running with Bun.
// PLAYWRIGHT_MODULE may point to an external installation; no dependency added.
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
	const route = (name) => page.getByRole('link', { name, exact: true }).click();
	const choose = async (name, value) => {
		await field(name).click();
		await page.getByRole('option', { name: value, exact: true }).click();
	};
	await page.goto(`${url}/projects`);
	await field('New workspace name').fill('Disposable projects smoke');
	await button('Create browser workspace').click();
	await button('New project').click();
	await field('Title').fill('Project smoke');
	await field('Markdown').fill('Project body 日本語 🦉');
	await button('Save project').click();
	await status('Saved to browser storage.');
	for (const value of [
		'active',
		'on-hold',
		'completed',
		'cancelled',
		'planned',
	]) {
		await choose('Status', value);
		await button('Save project').click();
		await status('Saved to browser storage.');
		await button(`Project smoke · ${value} · 0 tasks`).waitFor();
	}
	await field('Title').fill('Edited project');
	assert(await button('Move project').isDisabled());
	assert(await button('Move to trash').isDisabled());
	assert(await button('Download JSON backup').isDisabled());
	page.once('dialog', (dialog) => dialog.dismiss());
	await route('Tasks');
	assert.equal(await field('Title').inputValue(), 'Edited project');
	await button('Save project').click();
	await status('Saved to browser storage.');
	const projectId = (
		await page.getByRole('region', { name: 'Project tasks' }).innerText()
	).match(/Stable project ID: ([^\s]+)/)[1];
	await route('Tasks');
	await button('New task').click();
	await field('Title').fill('Linked task');
	await choose('Project', `Edited project · ${projectId}`);
	await button('Save task').click();
	await status('Saved to browser storage.');
	await route('Projects');
	await button('Edited project · planned · 1 tasks').click();
	await page
		.getByRole('region', { name: 'Project tasks' })
		.getByText('Linked task · todo', { exact: true })
		.waitFor();
	await field('Destination path').fill('Projects/Archive/project.md');
	await button('Move project').click();
	await status('Task relationships are unchanged.');
	await page.reload();
	await button('Open Disposable projects smoke').click();
	await button('Edited project · planned · 1 tasks').click();
	assert.equal(
		await field('Destination path').inputValue(),
		'Projects/Archive/project.md',
	);
	assert.equal(await field('Markdown').inputValue(), 'Project body 日本語 🦉');
	// External edit of only this disposable context's canonical project file.
	await page.evaluate(async () => {
		async function locate(dir) {
			try {
				await (
					await dir.getDirectoryHandle('.noura')
				).getFileHandle('workspace.yaml');
				return dir;
			} catch {
				/* Search OPFS containers. */
			}
			for await (const [, handle] of dir.entries()) {
				if (handle.kind === 'directory') {
					const found = await locate(handle);
					if (found) return found;
				}
			}
		}
		let dir = await locate(await navigator.storage.getDirectory());
		for (const part of ['Projects', 'Archive'])
			dir = await dir.getDirectoryHandle(part);
		const handle = await dir.getFileHandle('project.md');
		const text = await (await handle.getFile()).text();
		const writer = await handle.createWritable();
		await writer.write(
			text.replace('\n---\n', '\ncustom_smoke: retained\n---\n'),
		);
		await writer.close();
	});
	await field('Title').fill('Stale draft');
	await button('Save project').click();
	await page.getByRole('alert').filter({ hasText: 'Stale revision' }).waitFor();
	assert.equal(await field('Title').inputValue(), 'Stale draft');
	page.once('dialog', (dialog) => dialog.accept());
	await button('Discard draft / reload saved project').click();
	await page.waitForFunction(
		() => document.querySelector('#note-title').value === 'Edited project',
	);
	const download = page.waitForEvent('download');
	await button('Download JSON backup').click();
	const backupPath = await (await download).path();
	const backup = JSON.parse(await readFile(backupPath, 'utf8'));
	const canonicalProject = backup.entries.find(
		(entry) => entry.path === 'Projects/Archive/project.md',
	);
	assert.match(
		Buffer.from(canonicalProject.base64, 'base64').toString(),
		/custom_smoke: retained/,
	);
	const restoredContext = await browser.newContext();
	const restored = await restoredContext.newPage();
	restored.on('pageerror', (error) => errors.push(error.message));
	await restored.goto(`${url}/projects`);
	await restored.getByLabel('Import Noura JSON backup').waitFor();
	restored.once('dialog', (dialog) => dialog.accept());
	await restored
		.getByLabel('Import Noura JSON backup')
		.setInputFiles(backupPath);
	await restored
		.getByRole('button', {
			name: 'Edited project · planned · 1 tasks',
			exact: true,
		})
		.click();
	await restored
		.getByRole('region', { name: 'Project tasks' })
		.getByText(`Stable project ID: ${projectId}`, { exact: true })
		.waitFor();
	await restored
		.getByRole('region', { name: 'Project tasks' })
		.getByText('Linked task · todo', { exact: true })
		.waitFor();
	assert.equal(
		await restored.getByLabel('Destination path').inputValue(),
		'Projects/Archive/project.md',
	);
	page.once('dialog', (dialog) => {
		assert.match(
			dialog.message(),
			/Tasks and other files are not deleted or moved/,
		);
		return dialog.dismiss();
	});
	await button('Move to trash').click();
	await status('Move to trash cancelled');
	page.once('dialog', (dialog) => dialog.accept());
	await button('Move to trash').click();
	await status('No cascade');
	await route('Tasks');
	await button('Linked task · todo').click();
	assert.match(
		await field('Project').textContent(),
		new RegExp(`Unavailable project \\(${projectId}\\)`),
	);
	await field('Markdown').fill('Task survives project deletion');
	await button('Save task').click();
	await status('Saved to browser storage.');
	await page.reload();
	await button('Open Disposable projects smoke').click();
	await button('Linked task · todo').click();
	assert.match(await field('Project').textContent(), /Unavailable project/);
	assert.equal(
		await field('Markdown').inputValue(),
		'Task survives project deletion',
	);
	await choose('Project', 'No project');
	await button('Save task').click();
	await status('Saved to browser storage.');
	await button('Discard draft / reload saved task').click();
	assert.equal((await field('Project').textContent()).trim(), 'No project');
	assert.deepEqual(errors, []);
	console.log(
		'PASS: project create/edit/all statuses, dirty guards, task stable-ID membership, summaries/listTasks, move/reload, stale revision, backup/restore, trash cancellation/no cascade, missing membership preservation/clear; no page errors.',
	);
} finally {
	await browser.close();
}
