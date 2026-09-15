import adapter from '@sveltejs/adapter-static';

const hosted = process.env.NOURA_HOSTED_BUILD === '1';

export default {
	kit: {
		// Hosted pages retain the account/share CSP. Native builds keep Tauri's
		// own policy, including the IPC and local asset schemes it requires.
		...(hosted
			? {
					csp: {
						mode: 'hash',
						directives: {
							'default-src': ['self'],
							// The workspace format core runs as WebAssembly in a worker;
							// Chrome requires this directive to compile it under CSP.
							'script-src': ['self', 'wasm-unsafe-eval'],
							'connect-src': ['self'],
							'style-src': ['self', 'unsafe-inline'],
							'img-src': ['none'],
							'font-src': ['self'],
							'object-src': ['none'],
							'base-uri': ['none'],
							'form-action': ['self'],
						},
					},
				}
			: {}),
		adapter: adapter({
			fallback: 'index.html',
			pages: hosted ? 'build-hosted' : 'build',
			assets: hosted ? 'build-hosted' : 'build',
		}),
		alias: { $lib: './src/lib', $account: './src/lib/account' },
	},
};
