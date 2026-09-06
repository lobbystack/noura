import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig, type Plugin } from 'vite';
import { mkdirSync, writeFileSync } from 'node:fs';
import { isAbsolute } from 'node:path';

function browserLicenseInputs(): Plugin {
	return {
		name: 'noura-browser-license-inputs',
		apply: 'build',
		generateBundle(_options, bundle) {
			if (this.environment.name !== 'client') return;
			const rendered = new Set(
				Object.values(bundle).flatMap((entry) =>
					entry.type === 'chunk'
						? Object.entries(entry.modules)
								.filter(([, module]) => module.renderedLength > 0)
								.map(([id]) => id)
						: [],
				),
			);
			const inputs = [...this.getModuleIds()]
				.filter((id) => rendered.has(id))
				.map((id) => id.split('?')[0])
				.filter((id): id is string =>
					Boolean(id && isAbsolute(id) && id.includes('/node_modules/')),
				);
			const directory = new URL('./.svelte-kit/', import.meta.url);
			mkdirSync(directory, { recursive: true });
			writeFileSync(
				new URL('client-inputs.json', directory),
				JSON.stringify(
					{ version: 1, inputs: [...new Set(inputs)].sort() },
					null,
					2,
				) + '\n',
			);
		},
	};
}
export default defineConfig({
	plugins: [tailwindcss(), sveltekit(), browserLicenseInputs()],
	server: { proxy: { '/api': 'http://127.0.0.1:1900' } },
});
