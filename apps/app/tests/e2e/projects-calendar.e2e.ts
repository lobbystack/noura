import { expect, test } from './fixtures';

function today() {
	const now = new Date();
	const month = String(now.getMonth() + 1).padStart(2, '0');
	const day = String(now.getDate()).padStart(2, '0');
	return `${now.getFullYear()}-${month}-${day}`;
}

test.describe('projects', () => {
	test('a new project keeps its title and overview typed across saves', async ({
		app,
		page,
	}) => {
		await page.goto('/projects');
		await page.getByRole('button', { name: 'New project' }).first().click();
		const title = page.getByRole('textbox', { name: 'Project title' });
		await expect(title).toHaveValue('New project');
		await title.click();
		await page.keyboard.press('ControlOrMeta+a');
		await page.keyboard.type('Garden ');
		await page.waitForTimeout(400);
		await page.keyboard.type('plan');
		await app.editor('Project overview').click();
		let typed = '';
		for (const chunk of ['Beds and ', 'paths  ', 'first']) {
			await page.keyboard.type(chunk, { delay: 20 });
			typed += chunk;
			await page.waitForTimeout(350);
		}
		await page.keyboard.press('Enter');
		await page.keyboard.type('- dig');
		typed += '\n- dig';
		expect(await app.doc('Project overview')).toBe(typed);
		await expect(title).toHaveValue('Garden plan');
		const path = 'projects/new-project--';
		await expect
			.poll(async () => {
				const file = (await app.storedPaths()).find(
					(entry) => entry.startsWith(path) && entry.endsWith('project.md'),
				);
				const text = file ? await app.stored(file) : null;
				return text?.slice(text.indexOf('\n# '));
			})
			.toBe(`\n# Garden plan\n\n${typed}\n`);
		await page.getByLabel('Project status').click();
		await page.getByRole('option', { name: 'Active' }).click();
		await expect(page.getByLabel('Project status')).toHaveText('Active');
		expect(await app.doc('Project overview')).toBe(typed);
		expect(await app.messages()).toEqual([]);
	});
});

test.describe('calendar', () => {
	test('a task due today shows on the calendar and on Home, and opens', async ({
		app,
		page,
	}) => {
		await page.goto('/tasks?view=all');
		await page.getByRole('button', { name: 'New task' }).click();
		const title = page.getByRole('textbox', { name: 'Task title' });
		await title.click();
		await page.keyboard.press('ControlOrMeta+a');
		await page.keyboard.type('Dentist');
		await page.locator('input[type="date"]').fill(today());
		await expect
			.poll(async () => {
				const file = (await app.storedPaths()).find((entry) =>
					entry.startsWith('tasks/'),
				);
				return file ? await app.stored(file) : null;
			})
			.toMatch(new RegExp(`^due: '?${today()}'?$`, 'm'));
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Calendar' })
			.click();
		const entry = page.getByRole('main').getByText('Dentist');
		await expect(entry).toBeVisible();
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		const upcoming = page.getByRole('region', { name: 'Next 7 days' });
		await expect(upcoming.getByRole('link', { name: /Dentist/ })).toBeVisible();
		await upcoming.getByRole('link', { name: /Dentist/ }).click();
		await expect(title).toHaveValue('Dentist');
	});
});
