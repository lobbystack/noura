import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
import { pdfAssets } from './pdf-assets.js';

export default defineConfig({
	plugins: [pdfAssets(), tailwindcss(), sveltekit()],
	clearScreen: false,
	server: { strictPort: true },
	ssr: { noExternal: ['@noura/ai'] },
	build: {
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
