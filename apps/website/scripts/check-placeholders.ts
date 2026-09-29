/**
 * Fails when website source still contains bracketed placeholder text such as
 * `[Legal entity name]` or `[retention period, for example 30 days]`.
 *
 * Runs before `check` and `build`, so CI and deploys stop on unfinished copy.
 */
import { readdir, readFile } from 'node:fs/promises';
import { join, relative } from 'node:path';

const root = join(import.meta.dir, '..');
const sourceDir = join(root, 'src');
const extensions = ['.svelte', '.ts', '.js', '.html', '.css', '.json', '.md'];

/**
 * A bracket that opens on a letter and holds no code characters. Code such as
 * `matches[0]`, `[name, enabled]`, or `[data-state="open"]` never matches both
 * this and `looksLikeProse`.
 */
const bracketed = /\[([A-Za-zÀ-ÿ][^\][=`'"{}<>$;]*)\]/g;

function looksLikeProse(inner: string): boolean {
	const text = inner.trim();
	// Two words joined by a space that doesn't follow a comma, as in a sentence.
	if (/[^,\s]\s+\S/.test(text)) return true;
	// Single-word placeholders: [Placeholder], [TBD], [TODO].
	return /^[A-Z][A-Za-z]*$/.test(text);
}

async function* walk(dir: string): AsyncGenerator<string> {
	for (const entry of await readdir(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) yield* walk(path);
		else if (extensions.some((extension) => entry.name.endsWith(extension)))
			yield path;
	}
}

const problems: string[] = [];
for await (const file of walk(sourceDir)) {
	const text = await readFile(file, 'utf8');
	for (const match of text.matchAll(bracketed)) {
		if (!looksLikeProse(match[1])) continue;
		const line = text.slice(0, match.index).split('\n').length;
		const snippet = match[0].replace(/\s+/g, ' ');
		problems.push(`${relative(root, file)}:${line}  ${snippet}`);
	}
}

if (problems.length > 0) {
	console.error('Placeholder text found in apps/website/src:');
	for (const problem of problems) console.error(`  ${problem}`);
	console.error('Replace each bracketed value before publishing the site.');
	process.exit(1);
}
