import { expect, noteFileBody, test, type App } from './fixtures';

/** Today in the browser's time zone, as a task's `due` value. */
function today() {
	const now = new Date();
	const month = String(now.getMonth() + 1).padStart(2, '0');
	const day = String(now.getDate()).padStart(2, '0');
	return `${now.getFullYear()}-${month}-${day}`;
}

async function taskPath(app: App, title: string): Promise<string> {
	const slug = title.toLowerCase().replaceAll(' ', '-');
	let found: string | undefined;
	await expect
		.poll(async () => {
			found = (await app.storedPaths()).find((path) =>
				path.startsWith(`tasks/${slug}--`),
			);
			return found;
		})
		.toBeTruthy();
	return found!;
}

/** The value of `key` in a stored file's header, or null. */
function header(text: string | null, key: string): string | null {
	return text?.match(new RegExp(`^${key}: (.*)$`, 'm'))?.[1] ?? null;
}

async function openTasks(app: App, view = 'all') {
	await app.page.goto(`/tasks?view=${view}`);
	await expect(app.page.getByRole('heading', { name: 'Tasks' })).toBeVisible();
}

async function newTask(app: App, title: string) {
	await openTasks(app);
	await app.page.getByRole('button', { name: 'New task' }).click();
	const name = app.page.getByRole('textbox', { name: 'Task title' });
	await expect(name).toHaveValue('New task');
	await name.click();
	await app.page.keyboard.press('ControlOrMeta+a');
	await app.page.keyboard.type(title);
	// The file keeps the name it was created with; the title is inside it.
	return taskPath(app, 'New task');
}

test.describe('Home', () => {
	test('a task added for today shows under Due today and opens', async ({
		app,
		page,
	}) => {
		const input = page.getByRole('textbox', { name: 'Task' });
		await input.click();
		await page.keyboard.type('Buy milk');
		await page.keyboard.press('Enter');
		const today_ = page.getByRole('region', { name: 'Due today' });
		await expect(today_.getByRole('link', { name: 'Buy milk' })).toBeVisible();
		const path = await taskPath(app, 'Buy milk');
		expect(header(await app.stored(path), 'due')).toBe(today());
		await today_.getByRole('link', { name: 'Buy milk' }).click();
		await expect(page.getByRole('textbox', { name: 'Task title' })).toHaveValue(
			'Buy milk',
		);
		// Completing it from Home removes it from the list.
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		await today_.getByRole('button', { name: 'Complete Buy milk' }).click();
		await expect(today_.getByText('Nothing due today.')).toBeVisible();
		await expect
			.poll(async () => header(await app.stored(path), 'status'))
			.toBe('done');
	});

	test('recent notes open their note', async ({ app, page }) => {
		await app.newNote('Diary');
		await page.keyboard.type('dear diary');
		await expect
			.poll(() => app.storedBody('Diary.md'))
			.toBe(noteFileBody('dear diary'));
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		const recent = page.getByRole('region', { name: 'Recent' });
		await recent.getByRole('link', { name: 'Diary' }).click();
		await expect(app.fileName).toHaveValue('Diary');
		expect(await app.doc()).toBe('dear diary');
	});
});

test.describe('tasks', () => {
	test('typing a task’s title and body across saves loses nothing', async ({
		app,
		page,
	}) => {
		const path = await newTask(app, 'Write report');
		const body = app.editor('Task body');
		await body.click();
		let typed = '';
		for (const chunk of ['First line ', 'with spaces  ', 'and more ']) {
			await page.keyboard.type(chunk, { delay: 20 });
			typed += chunk;
			await page.waitForTimeout(350);
		}
		await page.keyboard.press('Enter');
		await page.keyboard.type('- item one');
		typed += '\n- item one';
		expect(await app.doc('Task body')).toBe(typed);
		await expect
			.poll(async () => {
				const text = await app.stored(path);
				return text?.slice(text.indexOf('# Write report'));
			})
			.toBe(`# Write report\n\n${typed}\n`);
		expect(await app.doc('Task body')).toBe(typed);
		await expect(page.getByRole('textbox', { name: 'Task title' })).toHaveValue(
			'Write report',
		);
		expect(await app.messages()).toEqual([]);
	});

	test.fixme('changing properties while typing keeps both', async ({ app, page }) => {
		const path = await newTask(app, 'Plan trip');
		const body = app.editor('Task body');
		await body.click();
		await page.keyboard.type('pack bags ');
		// A property change saves at once, in the middle of typing.
		await page.getByLabel('Status', { exact: true }).click();
		await page.getByRole('option', { name: /in progress/i }).click();
		await body.click();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type('and passport');
		await page.getByLabel('Priority', { exact: true }).click();
		await page.getByRole('option', { name: /high/i }).click();
		await expect
			.poll(async () => {
				const text = await app.stored(path);
				return [
					header(text, 'status'),
					header(text, 'priority'),
					text?.slice(text.indexOf('# Plan trip')),
				];
			})
			.toEqual([
				'in-progress',
				'high',
				'# Plan trip\n\npack bags and passport\n',
			]);
		expect(await app.doc('Task body')).toBe('pack bags and passport');
	});

	test('an outside edit to the open task merges with typing', async ({
		app,
		page,
	}) => {
		const path = await newTask(app, 'Shared task');
		await app.editor('Task body').click();
		await page.keyboard.type('local text');
		await expect
			.poll(async () => (await app.stored(path))?.endsWith('local text\n'))
			.toBe(true);
		const file = (await app.stored(path))!;
		await app.writeOutside(path, `${file}\nfrom outside\n`);
		await expect
			.poll(() => app.doc('Task body'))
			.toBe('local text\n\nfrom outside');
		await app.caretToEnd('Task body');
		await page.keyboard.type('!');
		await expect
			.poll(async () => (await app.stored(path))?.endsWith('from outside!\n'))
			.toBe(true);
		expect(await app.stored(path)).toContain('local text\n\nfrom outside!');
	});
});
