import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { createBrowserApp } from '../src/web';

const build = new URL('../../app/build-hosted/', import.meta.url);
const html = await Bun.file(new URL('index.html', build)).text();
assert.match(html, /http-equiv="content-security-policy"/i);
for (const directive of [
	"default-src 'self'",
	"script-src 'self'",
	"connect-src 'self'",
	"img-src 'none'",
	"object-src 'none'",
	"base-uri 'none'",
	"form-action 'self'",
])
	assert.ok(html.includes(directive), `Missing hosted CSP: ${directive}`);
assert.ok(!html.includes("'unsafe-eval'"));
for (const [, attributes, script] of html.matchAll(
	/<script([^>]*)>([\s\S]*?)<\/script>/g,
)) {
	if (attributes?.includes('src=') || !script?.trim()) continue;
	const hash = createHash('sha256').update(script).digest('base64');
	assert.ok(
		html.includes(`sha256-${hash}`),
		'Bootstrap script must be CSP-hashed',
	);
}

type Chunk = { file: string; imports: string[]; modules: string[] };
const { chunks } = (await Bun.file(
	new URL('../../app/.svelte-kit/hosted-client-inputs.json', import.meta.url),
).json()) as { chunks: Chunk[] };
const byFile = new Map(chunks.map((chunk) => [chunk.file, chunk]));
const accountChunks = chunks.filter((chunk) =>
	chunk.modules.some(
		(id) =>
			id.includes('/src/routes/(account)/') ||
			id.endsWith('/src/routes/+layout.svelte'),
	),
);
for (const route of [
	'+layout.svelte',
	'(account)/+layout.svelte',
	'(account)/account/+page.svelte',
	'(account)/account/device/+page.svelte',
	'(account)/invite/[token]/+page.svelte',
	'(account)/share/[token]/+page.svelte',
])
	assert.ok(
		accountChunks.some((chunk) =>
			chunk.modules.some((id) => id.endsWith(`/src/routes/${route}`)),
		),
		`Missing browser route or layout: ${route}`,
	);
const visited = new Set<string>();
function verifyBoundary(chunk: Chunk) {
	if (visited.has(chunk.file)) return;
	visited.add(chunk.file);
	for (const id of chunk.modules)
		assert.ok(
			!/\/src\/lib\/(?:state\.svelte|plugins\.svelte|ai\/)|@tauri-apps|\/src\/routes\/\(workspace\)\//.test(
				id,
			),
			`Native initialization leaked into account route: ${id}`,
		);
	for (const file of chunk.imports) {
		const dependency = byFile.get(file);
		assert.ok(dependency, `Missing browser chunk: ${file}`);
		verifyBoundary(dependency);
	}
}
accountChunks.forEach(verifyBoundary);

const app = createBrowserApp(fileURLToPath(build));
for (const path of [
	'/',
	'/account',
	'/account/device?user_code=ABCD1234',
	'/invite/token',
	'/share/token',
]) {
	const response = await app.request(path);
	assert.equal(response.status, 200);
	assert.equal(await response.text(), html);
}
for (const chunk of chunks) {
	const response = await app.request(`/${chunk.file}`);
	assert.equal(response.status, 200, `Missing served asset: ${chunk.file}`);
	assert.ok(!response.headers.get('content-type')?.includes('text/html'));
}
const wasmRoot = new URL('workspace-wasm/', build);
function walkWasm(directory: string): string[] {
	const entries = readdirSync(new URL(`${directory}/`, wasmRoot), {
		withFileTypes: true,
	});
	return entries.flatMap((entry) =>
		entry.isDirectory()
			? walkWasm(`${directory}/${entry.name}`)
			: [`${directory}/${entry.name}`],
	);
}
let sawWasmBinary = false;
for (const file of walkWasm('.')) {
	const normalized = file.replace(/^\.\//, '');
	if (normalized.endsWith('workspace_format_wasm_bg.wasm'))
		sawWasmBinary = true;
	const response = await app.request(`/workspace-wasm/${normalized}`);
	assert.equal(
		response.status,
		200,
		`Missing served workspace wasm asset: ${normalized}`,
	);
}
assert.ok(sawWasmBinary, 'Hosted build is missing the workspace wasm binary');
assert.ok(
	html.includes('wasm-unsafe-eval'),
	'Hosted CSP must allow WebAssembly compilation',
);
for (const path of [
	'/api/missing',
	'/v1/missing',
	'/public/missing',
	'/_app/missing.js',
	'/workspace-wasm/missing.wasm',
	'/unknown',
])
	assert.equal((await app.request(path)).status, 404);
console.info(
	'Verified hosted CSP, route isolation, SPA responses, browser chunks, and workspace wasm assets.',
);
