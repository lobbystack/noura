import { defineConfig, devices } from '@playwright/test';

const port = Number(process.env.NOURA_E2E_PORT ?? 4317);
const baseURL = `http://localhost:${port}`;

/**
 * End-to-end tests drive the browser build of the app: the same shell,
 * routes and editor as the desktop app, backed by the browser workspace
 * worker and its origin-private file system. Every test gets a fresh browser
 * context, so every test starts from an empty workspace list.
 */
export default defineConfig({
	testDir: './tests/e2e',
	testMatch: '**/*.e2e.ts',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	retries: 0,
	workers: process.env.CI ? 2 : undefined,
	timeout: 60_000,
	expect: { timeout: 10_000 },
	reporter: process.env.CI ? [['list'], ['github']] : 'list',
	use: {
		baseURL,
		trace: 'retain-on-failure',
	},
	projects: [
		{
			name: 'chromium',
			use: {
				...devices['Desktop Chrome'],
				viewport: { width: 1400, height: 900 },
			},
		},
	],
	webServer: {
		command: `bun run build && bun x vite preview --port ${port} --strictPort`,
		url: baseURL,
		reuseExistingServer: !process.env.CI,
		timeout: 180_000,
		stdout: 'ignore',
		stderr: 'pipe',
	},
});
