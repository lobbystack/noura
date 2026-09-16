// Run with Bun against the running app; uses fresh contexts, never the user's profile.
import assert from 'node:assert/strict';
const { chromium } = await import(
	process.env.PLAYWRIGHT_MODULE || 'playwright'
);
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const url = process.env.NOURA_SMOKE_URL || 'http://127.0.0.1:4187';
try {
	const context = await browser.newContext();
	const page = await context.newPage();
	const errors = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const button = (name) => page.getByRole('button', { name, exact: true });
	const link = (name) => page.getByRole('link', { name, exact: true });
	const toggle = (name) =>
		page.getByRole('switch', { name: `Enable ${name}`, exact: true });
	const settled = () =>
		page.waitForFunction(
			() => !document.querySelector('main')?.textContent?.includes('Working…'),
		);
	const navigate = async (name) => {
		await settled();
		await link(name).click();
		try {
			await page.waitForURL(
				`${url}/${name === 'Plugin settings' ? 'settings' : name.toLowerCase()}`,
			);
		} catch (cause) {
			throw new Error(
				`Navigation to ${name} from ${page.url()}: ${await page.locator('main').innerText()}`,
				{ cause },
			);
		}
		await settled();
	};
	await page.goto(`${url}/tasks`);
	await page
		.getByLabel('New workspace name', { exact: true })
		.fill('Plugin smoke');
	await button('Create browser workspace').click();
	await settled();
	// New manifests start with no enabled plugins; configure through canonical settings.
	await navigate('Plugin settings');
	for (const name of ['Notes', 'Tasks', 'Projects']) {
		await toggle(name).click();
		await page.waitForFunction(
			(name) =>
				document
					.querySelector(`[aria-label="Enable ${name}"]`)
					?.getAttribute('aria-checked') === 'true',
			name,
		);
		await link(name).waitFor();
	}
	await navigate('Tasks');
	await button('New task').click();
	await page.getByLabel('Title', { exact: true }).fill('Retained plugin draft');
	await page.getByLabel('Markdown', { exact: true }).fill('Draft body 日本語');
	await navigate('Notes');
	await navigate('Tasks');
	assert.equal(
		await page.getByLabel('Title', { exact: true }).inputValue(),
		'Retained plugin draft',
	);
	await navigate('Plugin settings');
	await toggle('Tasks').click();
	await page.waitForFunction(
		() =>
			document
				.querySelector('[aria-label="Enable Tasks"]')
				?.getAttribute('aria-checked') === 'false',
	);
	await link('Tasks').waitFor({ state: 'hidden' });
	assert.equal(await link('Tasks').count(), 0);
	assert(
		await page
			.getByText('Unsaved drafts are retained', { exact: false })
			.count(),
	);
	for (const name of ['Calendar', 'AI', 'Folders']) {
		assert(await toggle(name).isDisabled());
		const row = page.getByRole('listitem').filter({
			has: page.getByRole('button', {
				name: `${name} settings`,
				exact: true,
			}),
		});
		assert.equal((await row.locator('svg').count()) > 0, true);
		assert.equal(
			await row
				.getByTitle('Web: Not supported', { exact: true })
				.locator('svg')
				.count(),
			1,
		);
		assert.equal(
			await row
				.getByTitle('Desktop: Supported', { exact: true })
				.locator('svg')
				.count(),
			1,
		);
	}
	const tasksRow = page
		.getByRole('listitem')
		.filter({ has: button('Tasks settings') });
	// Shared platform component advertises web support only for web-capable manifests.
	assert.equal(
		await tasksRow
			.getByTitle('Web: Supported', { exact: true })
			.locator('svg')
			.count(),
		1,
	);
	const other = await context.newPage();
	await other.goto(`${url}/tasks`);
	await other
		.getByRole('button', { name: 'Open Plugin smoke', exact: true })
		.click();
	await other
		.getByText('This plugin is disabled or not active', { exact: false })
		.waitFor();
	assert.equal(
		await other.getByLabel('Task editor', { exact: true }).count(),
		0,
	);
	// Canonical cross-tab refresh: enabling here must reactivate the first tab too.
	await other
		.getByRole('link', { name: 'Plugin settings', exact: true })
		.click();
	await other
		.getByRole('switch', { name: 'Enable Tasks', exact: true })
		.click();
	await link('Tasks').waitFor();
	await navigate('Tasks');
	assert.equal(
		await page.getByLabel('Title', { exact: true }).inputValue(),
		'Retained plugin draft',
	);
	assert.equal(
		await page.getByLabel('Markdown', { exact: true }).inputValue(),
		'Draft body 日本語',
	);
	await button('Save task').click();
	await settled();
	await page
		.getByRole('status')
		.filter({ hasText: 'Saved to browser storage.' })
		.waitFor();
	await navigate('Plugin settings');
	await toggle('Tasks').click();
	await page.waitForFunction(
		() =>
			document
				.querySelector('[aria-label="Enable Tasks"]')
				?.getAttribute('aria-checked') === 'false',
	);
	await link('Tasks').waitFor({ state: 'hidden' });
	const downloadPromise = page.waitForEvent('download');
	await button('Download JSON backup').click();
	const download = await downloadPromise;
	const restoredContext = await browser.newContext();
	const restored = await restoredContext.newPage();
	restored.on('pageerror', (error) => errors.push(error.message));
	await restored.goto(`${url}/tasks`);
	restored.once('dialog', (dialog) => dialog.accept());
	await restored
		.getByLabel('Import Noura JSON backup', { exact: true })
		.setInputFiles(await download.path());
	await restored
		.getByRole('status')
		.filter({ hasText: 'Backup imported and opened.' })
		.waitFor();
	await restored
		.getByText('This plugin is disabled or not active', { exact: false })
		.waitFor();
	assert.equal(
		await restored.getByRole('link', { name: 'Tasks', exact: true }).count(),
		0,
	);
	await restored.getByRole('link', { name: 'Notes', exact: true }).waitFor();
	await restoredContext.close();
	await page.reload();
	await button('Open Plugin smoke').click();
	await settled();
	assert.equal(await toggle('Tasks').getAttribute('aria-checked'), 'false');
	assert.equal(await link('Tasks').count(), 0);
	await page
		.getByLabel('New workspace name', { exact: true })
		.fill('Independent empty preferences');
	await button('Create browser workspace').click();
	await settled();
	assert.equal(await link('Notes').count(), 0);
	assert.equal(await link('Tasks').count(), 0);
	await page.goto(`${url}/tasks`);
	await button('Open Plugin smoke').click();
	await page
		.getByText('This plugin is disabled or not active', { exact: false })
		.waitFor();
	assert.equal(await button('New task').count(), 0);
	await navigate('Plugin settings');
	await toggle('Tasks').click();
	await navigate('Tasks');
	await button('Rebuild / refresh tasks').click();
	await button('Retained plugin draft · todo').click();
	assert.equal(
		await page.getByLabel('Markdown', { exact: true }).inputValue(),
		'Draft body 日本語',
	);
	for (const route of ['calendar', 'ai', 'pdf']) {
		await page.goto(`${url}/${route}`);
		await button('Open Plugin smoke').click();
		await page
			.getByText('This feature is not supported in browser workspaces.', {
				exact: false,
			})
			.waitFor();
		assert.equal(
			await page.getByLabel('Note editor', { exact: true }).count(),
			0,
		);
	}
	await button('Close workspace').click();
	await settled();
	assert.equal(await link('Notes').count(), 0);
	assert.equal(await link('Tasks').count(), 0);
	assert.deepEqual(errors, []);
	await context.close();
	console.log(
		'PASS: fresh browser runtime, draft retention, disable/reload/direct URL, cross-tab reenable, canonical backup import, independent create, unsupported settings/routes, close cleanup',
	);
} finally {
	await browser.close();
}
