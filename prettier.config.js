export default {
	useTabs: true,
	singleQuote: true,
	trailingComma: 'all',
	// Prose paragraphs and list items render as one source line so editors wrap
	// them, per the writing guidelines.
	proseWrap: 'never',
	plugins: ['prettier-plugin-svelte'],
	overrides: [{ files: '*.svelte', options: { parser: 'svelte' } }],
};
