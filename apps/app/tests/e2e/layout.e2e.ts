import { expect, test } from './fixtures';

test.describe('layout at a normal window width', () => {
	test.use({ viewport: { width: 1100, height: 750 } });

	test('the task pane shows its whole toolbar and readable values', async ({
		app,
		page,
	}) => {
		await page.goto('/tasks?view=all');
		await page.getByRole('button', { name: 'New task' }).click();
		const toolbar = page.getByRole('toolbar', { name: 'Formatting' });
		await expect(toolbar).toBeVisible();
		const overflow = await toolbar.evaluate(
			(element) => element.scrollWidth - element.clientWidth,
		);
		expect(overflow).toBeLessThanOrEqual(0);
		await expect(
			toolbar.getByRole('button', { name: /Find and replace/ }),
		).toBeInViewport({ ratio: 1 });
		await expect(page.getByLabel('Status', { exact: true })).toHaveText(
			'To do',
		);
		await expect(page.getByLabel('Priority', { exact: true })).toHaveText(
			'Medium',
		);
		await page.getByLabel('Status', { exact: true }).click();
		await expect(page.getByRole('option')).toHaveText([
			'To do',
			'In progress',
			'Done',
			'Cancelled',
		]);
		await page.keyboard.press('Escape');
		expect(await app.storedPaths()).toHaveLength(2);
	});

	test('tabs fit the title bar row with no scrollbar and still scroll', async ({
		app,
		page,
	}) => {
		for (let index = 1; index <= 12; index += 1)
			await app.newNote(`A long note name ${index}`);
		await page
			.getByRole('navigation', { name: 'Sections' })
			.getByRole('link', { name: 'Home' })
			.click();
		const bar = page.getByRole('tablist', { name: 'Open files' });
		const box = await bar.evaluate((element: HTMLElement) => {
			const header = document.querySelector('header')!.getBoundingClientRect();
			const rect = element.getBoundingClientRect();
			const tab = element
				.querySelector('[role="tab"]')!
				.getBoundingClientRect();
			return {
				top: rect.top,
				bottom: rect.bottom,
				headerBottom: header.bottom,
				scrollbar:
					element.offsetHeight -
					element.clientHeight -
					parseFloat(getComputedStyle(element).borderBottomWidth),
				// Some systems draw overlay scrollbars that take no space.
				hidden: getComputedStyle(element).scrollbarWidth === 'none',
				overflowing: element.scrollWidth > element.clientWidth,
				tabTop: tab.top,
				tabBottom: tab.bottom,
			};
		});
		expect(box.top).toBe(0);
		expect(box.bottom).toBe(box.headerBottom);
		expect(box.scrollbar).toBe(0);
		expect(box.hidden).toBe(true);
		expect(box.overflowing).toBe(true);
		expect(box.tabTop).toBeGreaterThan(box.top);
		expect(box.tabBottom).toBeLessThan(box.bottom);
		// A mouse wheel scrolls the tabs sideways.
		await bar.hover();
		await page.mouse.wheel(0, 400);
		await expect
			.poll(() => bar.evaluate((element) => element.scrollLeft))
			.toBeGreaterThan(0);
	});
});
