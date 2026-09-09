import { describe, expect, test } from 'bun:test';
import { readdir, readFile } from 'node:fs/promises';
import { extname, join, relative, resolve } from 'node:path';

const repositoryRoot = resolve(import.meta.dir, '../../../..');
const sourceRoots = [
	resolve(repositoryRoot, 'apps/app/src'),
	resolve(repositoryRoot, 'packages/editor/src'),
];
const sourceExtensions = new Set(['.css', '.svelte', '.ts']);
const forbiddenTextSize =
	/\btext-(?:lg|xl|[2-9]xl|\[(?:\d*\.?\d+)(?:px|rem|em)\])\b/g;
const fontSizeDeclaration = /(?:font-size|fontSize)\s*:/;
const allowedFontSizeVariable =
	/var\(--(?:text-(?:xs|sm|base)|content-(?:text|heading-[1-4])-size)(?:,[^)]+)?\)/;

async function sourceFiles(directory: string): Promise<string[]> {
	const entries = await readdir(directory, { withFileTypes: true });
	const files = await Promise.all(
		entries.map(async (entry) => {
			const path = join(directory, entry.name);
			if (entry.isDirectory()) return sourceFiles(path);
			if (
				entry.name === 'typography.test.ts' ||
				!sourceExtensions.has(extname(entry.name))
			)
				return [];
			return [path];
		}),
	);
	return files.flat();
}

describe('application typography', () => {
	test('keeps UI and Markdown content on their separate type scales', async () => {
		const files = (await Promise.all(sourceRoots.map(sourceFiles))).flat();
		const violations: string[] = [];

		for (const path of files) {
			const source = await readFile(path, 'utf8');
			for (const match of source.matchAll(forbiddenTextSize)) {
				violations.push(`${relative(repositoryRoot, path)}: ${match[0]}`);
			}
			for (const [index, line] of source.split('\n').entries()) {
				if (
					fontSizeDeclaration.test(line) &&
					!allowedFontSizeVariable.test(line)
				)
					violations.push(
						`${relative(repositoryRoot, path)}:${index + 1}: ${line.trim()}`,
					);
			}
		}

		expect(violations).toEqual([]);
	});
});
