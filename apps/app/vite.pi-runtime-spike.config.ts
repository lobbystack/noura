import { resolve } from 'node:path';
import { defineConfig } from 'vite';

export default defineConfig({
	build: {
		lib: {
			entry: resolve(import.meta.dirname, 'src/lib/pi-runtime-spike-entry.ts'),
			formats: ['es'],
			fileName: 'noura-pi-runtime-spike',
		},
		outDir: 'dist/pi-runtime-spike',
		emptyOutDir: true,
		target: 'es2022',
	},
});
