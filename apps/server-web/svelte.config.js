import adapter from '@sveltejs/adapter-static';
export default {
	kit: {
		csp: {
			mode: 'hash',
			directives: {
				'default-src': ['self'],
				'script-src': ['self'],
				'connect-src': ['self'],
				'style-src': ['self', 'unsafe-inline'],
				'img-src': ['none'],
				'font-src': ['self'],
				'object-src': ['none'],
				'base-uri': ['none'],
				'form-action': ['self'],
			},
		},
		adapter: adapter({ fallback: 'index.html' }),
		files: { lib: '../app/src/lib' },
		alias: { $account: './src/lib' },
	},
};
