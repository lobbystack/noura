import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import type { Plugin } from 'vite';

/** Keep wasm-bindgen's glue, WASM, and generated JS snippets together. */
export function workspaceWasmAssets(): Plugin {
	const root = join(
		import.meta.dirname,
		'../../packages/workspace-format-wasm/wasm',
	);
	const files = new Map<string, Uint8Array>();
	function collect(directory = '') {
		for (const entry of readdirSync(join(root, directory), {
			withFileTypes: true,
		})) {
			const path = directory ? `${directory}/${entry.name}` : entry.name;
			if (entry.isDirectory()) collect(path);
			else if (/\.(js|wasm)$/.test(path))
				files.set(path, readFileSync(join(root, path)));
		}
	}
	collect();
	const hash = createHash('sha256');
	for (const [path, bytes] of [...files].sort(([a], [b]) => a.localeCompare(b)))
		hash.update(path).update(bytes);
	const prefix = `workspace-wasm/${hash.digest('hex').slice(0, 16)}/`;
	return {
		name: 'noura-workspace-wasm-assets',
		config: () => ({
			define: {
				__NOURA_WORKSPACE_WASM_URL__: JSON.stringify(
					`/${prefix}workspace_format_wasm.js`,
				),
			},
		}),
		configureServer(server) {
			server.middlewares.use((request, response, next) => {
				const path = request.url?.split('?')[0] ?? '';
				const bytes = path.startsWith(`/${prefix}`)
					? files.get(path.slice(prefix.length + 1))
					: undefined;
				if (!bytes) return next();
				response.setHeader(
					'Content-Type',
					path.endsWith('.wasm') ? 'application/wasm' : 'text/javascript',
				);
				response.end(bytes);
			});
		},
		generateBundle() {
			for (const [path, source] of files)
				this.emitFile({ type: 'asset', fileName: prefix + path, source });
		},
	};
}
