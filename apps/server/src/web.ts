import { Hono } from 'hono';
import { serveStatic } from 'hono/bun';
import { join } from 'node:path';

/** Explicit SPA routes: never turn missing APIs or assets into successful HTML. */
export function createBrowserApp(webRoot: string) {
	const app = new Hono();
	app.get('/_app/*', serveStatic({ root: webRoot }));
	app.get('/pdfjs/*', serveStatic({ root: webRoot }));
	app.get('/workspace-wasm/*', serveStatic({ root: webRoot }));
	const routes = [
		'/',
		'/inbox',
		'/notes',
		'/tasks',
		'/calendar',
		'/projects',
		'/ai',
		'/pdf',
		'/settings',
		'/account',
		'/account/device',
		'/invite/:token',
		'/share/:token',
	];
	for (const route of routes) {
		app.get(route, async (c) => {
			c.header('Cache-Control', 'no-store');
			c.header('Referrer-Policy', 'no-referrer');
			c.header('Content-Security-Policy', "frame-ancestors 'none'");
			const file = Bun.file(join(webRoot, 'index.html'));
			if (!(await file.exists()))
				return c.json({ error: { code: 'server.web_unavailable' } }, 503);
			return c.html(await file.text());
		});
	}
	return app;
}
