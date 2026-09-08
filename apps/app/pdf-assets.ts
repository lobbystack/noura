import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';
import type { Plugin } from 'vite';

/** Package assets are emitted verbatim, including their license files. */
export function pdfAssets(): Plugin {
	const root = dirname(
		createRequire(import.meta.url).resolve('pdfjs-dist/package.json'),
	);
	const assets = new Map<string, Uint8Array>();
	for (const directory of [
		'cmaps',
		'standard_fonts',
		'wasm',
		'iccs',
		'web/images',
	]) {
		for (const file of readdirSync(join(root, directory), {
			withFileTypes: true,
		})) {
			if (file.name.startsWith('quickjs-eval')) continue;
			if (
				directory === 'standard_fonts' &&
				(file.name.startsWith('Liberation') ||
					file.name === 'LICENSE_LIBERATION')
			)
				continue;
			if (file.isFile())
				assets.set(
					`pdfjs/${directory}/${file.name}`,
					readFileSync(join(root, directory, file.name)),
				);
		}
	}
	for (const file of readdirSync(
		join(import.meta.dirname, 'vendor/pdf-fonts'),
	)) {
		assets.set(
			`pdfjs/standard_fonts/${file === 'LICENSE' ? 'LICENSE_LIBERATION_OFL' : file}`,
			readFileSync(join(import.meta.dirname, 'vendor/pdf-fonts', file)),
		);
	}
	assets.set('pdfjs/LICENSE', readFileSync(join(root, 'LICENSE')));
	return {
		name: 'noura-pdf-assets',
		configureServer(server) {
			server.middlewares.use((request, response, next) => {
				const key = request.url?.split('?')[0]?.replace(/^\//, '') ?? '';
				const data = assets.get(key);
				if (!data) return next();
				response.setHeader(
					'Content-Type',
					key.endsWith('.wasm')
						? 'application/wasm'
						: key.endsWith('.svg')
							? 'image/svg+xml'
							: 'application/octet-stream',
				);
				response.end(data);
			});
		},
		generateBundle() {
			for (const [fileName, source] of assets)
				this.emitFile({ type: 'asset', fileName, source });
		},
	};
}
