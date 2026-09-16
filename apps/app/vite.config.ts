import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
import { pdfAssets } from './pdf-assets.js';
import { browserLicenseInputs } from './browser-license-inputs.js';
import { workspaceWasmAssets } from './workspace-wasm-assets.js';

export default defineConfig({
	worker: { format: 'es' },
	// Only expose the target, not the rest of the build environment.
	define: {
		'import.meta.env.NOURA_TAURI_PLATFORM': JSON.stringify(
			process.env.TAURI_ENV_PLATFORM ?? '',
		),
	},
	plugins: [
		workspaceWasmAssets(),
		pdfAssets(),
		tailwindcss(),
		sveltekit(),
		browserLicenseInputs(),
	],
	clearScreen: false,
	server: {
		strictPort: true,
		proxy: {
			'/api': 'http://127.0.0.1:1900',
			'/public': 'http://127.0.0.1:1900',
			'/v1': 'http://127.0.0.1:1900',
		},
	},
	ssr: { noExternal: ['@noura/ai'] },
	build: {
		assetsInlineLimit: 0,
		rolldownOptions: {
			output: {
				codeSplitting: {
					groups: [
						{
							name: 'editor-codemirror',
							test: (id) => id.includes('@codemirror') || id.includes('@lezer'),
							priority: 30,
						},
						{
							name: 'editor-yjs',
							test: (id) =>
								id.includes('y-codemirror.next') ||
								id.includes('/yjs/') ||
								id.includes('/lib0/'),
							priority: 20,
						},
						{
							name: 'editor-katex',
							test: (id) => id.includes('/katex/'),
							priority: 20,
						},
					],
				},
			},
		},
	},
});
