import type { Locator, Page } from '@playwright/test';
import { expect, test, type App } from './fixtures';

const DOC = [
	'> [!note] Remember',
	'> Callout body',
	'',
	'- [ ] open',
	'- [x] done',
	'',
	'```ts',
	'const a = 1;',
	'let text = "hi"; // note',
	'```',
	'',
	'Last line',
].join('\n');

async function openRendered(app: App) {
	await app.openFiles();
	await app.writeOutside('Rendered.md', DOC);
	await app.treeItem('Rendered').click();
	await expect(app.editor('File text')).toBeVisible();
	// Keep the caret on the last line so every block above renders.
	await app.caretAfter('Last line', 'File text');
}

/** WCAG contrast of two CSS colors, resolved by the browser. */
async function contrast(page: Page, foreground: Locator, background: Locator) {
	const colors = [
		await foreground.evaluate((element) => getComputedStyle(element).color),
		await background.evaluate(
			(element) => getComputedStyle(element).backgroundColor,
		),
	];
	return page.evaluate((colors) => {
		const canvas = document.createElement('canvas');
		canvas.width = canvas.height = 1;
		const context = canvas.getContext('2d', { willReadFrequently: true })!;
		const luminance = (color: string) => {
			context.clearRect(0, 0, 1, 1);
			context.fillStyle = color;
			context.fillRect(0, 0, 1, 1);
			const [r, g, b] = [...context.getImageData(0, 0, 1, 1).data].map(
				(value) => {
					const channel = value / 255;
					return channel <= 0.03928
						? channel / 12.92
						: ((channel + 0.055) / 1.055) ** 2.4;
				},
			);
			return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
		};
		const [first, second] = colors.map(luminance) as [number, number];
		return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
	}, colors);
}

for (const colorScheme of ['light', 'dark'] as const) {
	test.describe(`rendered Markdown, ${colorScheme}`, () => {
		test.use({ colorScheme });

		test('code keeps readable colors', async ({ app, page }) => {
			await openRendered(app);
			const editor = app.editor('File text');
			const block = editor.locator('.cm-md-codeblock').nth(1);
			for (const token of ['.tok-keyword', '.tok-definition', '.tok-string'])
				expect(
					await contrast(
						page,
						editor.locator(`.cm-md-codeblock ${token}`).first(),
						block,
					),
					`${token} in ${colorScheme}`,
				).toBeGreaterThanOrEqual(4.5);
		});

		test('a checked checkbox shows a check mark', async ({ app, page }) => {
			await openRendered(app);
			const box = app
				.editor('File text')
				.locator('.cm-md-checkbox[aria-checked="true"]');
			await expect(box).toBeVisible();
			const mark = await box.evaluate((element) => {
				const style = getComputedStyle(element, '::after');
				return {
					mask: style.maskImage || style.webkitMaskImage,
					color: style.backgroundColor,
					fill: getComputedStyle(element).backgroundColor,
				};
			});
			expect(mark.mask).toContain('url(');
			const ratio = await page.evaluate(
				({ color, fill }) => {
					const canvas = document.createElement('canvas');
					const context = canvas.getContext('2d')!;
					const read = (value: string) => {
						context.fillStyle = value;
						context.fillRect(0, 0, 1, 1);
						return [...context.getImageData(0, 0, 1, 1).data];
					};
					const [a, b] = [read(color), read(fill)];
					return Math.abs(a[0]! - b[0]!) + Math.abs(a[1]! - b[1]!);
				},
				{ color: mark.color, fill: mark.fill },
			);
			expect(ratio).toBeGreaterThan(200);
		});

		test('a callout hides its markers and names its type', async ({ app }) => {
			await openRendered(app);
			const lines = app.editor('File text').locator('.cm-md-callout');
			// What people see, with text-transform applied.
			const shown = (index: number) =>
				lines.nth(index).evaluate((element) => element.innerText);
			await expect.poll(() => shown(0)).toBe('Note Remember');
			await expect.poll(() => shown(1)).toBe('Callout body');
			// With the caret inside, the source shows again.
			await app.caretAfter('Callout', 'File text');
			await expect.poll(() => shown(0)).toBe('> [!note] Remember');
			await expect.poll(() => shown(1)).toBe('> Callout body');
		});
	});
}
