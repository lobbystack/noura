import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createBrowserApp } from './web';
import { createApp } from './app';
import type { SyncStore } from './store';

let root: string;
const html =
	'<!doctype html><meta http-equiv="Content-Security-Policy" content="script-src \'self\'"><title>Noura</title>';
beforeAll(async () => {
	root = await mkdtemp(join(tmpdir(), 'noura-web-'));
	await mkdir(join(root, '_app'));
	await writeFile(join(root, 'index.html'), html);
	await writeFile(join(root, '_app', 'test.js'), 'export {};');
});
afterAll(async () => {
	await rm(root, { recursive: true, force: true });
});

describe('unified browser static routing', () => {
	test('serves each SPA destination with security headers', async () => {
		const app = createBrowserApp(root);
		for (const path of [
			'/',
			'/inbox',
			'/notes',
			'/tasks',
			'/calendar',
			'/projects',
			'/ai',
			'/pdf',
			'/settings',
			'/account?mode=signup',
			'/account/device?user_code=ABCD1234',
			'/invite/token',
			'/share/token',
		]) {
			const response = await app.request(path);
			expect(response.status).toBe(200);
			expect(await response.text()).toBe(html);
			expect(response.headers.get('Referrer-Policy')).toBe('no-referrer');
			expect(response.headers.get('Content-Security-Policy')).toBe(
				"frame-ancestors 'none'",
			);
			expect(response.headers.get('Cache-Control')).toBe('no-store');
		}
	});
	test('missing APIs, unknown routes, assets and mutations never get the SPA', async () => {
		const app = createApp({} as SyncStore, {
			origin: 'http://localhost',
			webRoot: root,
		});
		for (const path of [
			'/api/missing',
			'/v1/missing',
			'/public/missing/extra',
			'/unknown',
			'/account/unknown',
			'/share/token/extra',
			'/_app/missing.js',
			'/pdfjs/missing.wasm',
			'/_app/%2e%2e%2findex.html',
		]) {
			const response = await app.request(path);
			expect(response.status).toBeGreaterThanOrEqual(400);
			expect(await response.text()).not.toBe(html);
		}
		expect((await app.request('/account', { method: 'POST' })).status).toBe(
			404,
		);
	});
	test('serves real assets, HEAD, and reports a missing build', async () => {
		const app = createBrowserApp(root);
		const asset = await app.request('/_app/test.js');
		expect(asset.status).toBe(200);
		expect(await asset.text()).toBe('export {};');
		const head = await app.request('/account', { method: 'HEAD' });
		expect(head.status).toBe(200);
		expect(await head.text()).toBe('');
		expect(
			(await createBrowserApp(join(root, 'missing')).request('/account'))
				.status,
		).toBe(503);
	});
});
