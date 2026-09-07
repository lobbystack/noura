/** UI routing hint only; native opening validates UTF-8, size and permissions. */
export function isPlainTextPath(path: string): boolean {
	const name = path.slice(path.lastIndexOf('/') + 1);
	return (
		/\.(txt|json|jsonc|ts|tsx|js|jsx|mjs|cjs|svelte|rs|toml|ya?ml|css|scss|html?|xml|csv|sql|py|sh|mdx|c|cpp|h|go|java|rb|php|swift|kt|ini|conf|log|vue)$/i.test(
			name,
		) || /^(Makefile|Dockerfile|LICENSE|NOTICE)$/i.test(name)
	);
}
