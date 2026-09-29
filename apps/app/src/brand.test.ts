import { describe, expect, test } from 'bun:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';

/**
 * The brand is written "noura" in every string people read. Only the macOS
 * menu bar keeps "Noura", and that comes from tauri.conf.json, not from here.
 * Identifiers such as getNouraClient and NOURA_* are not copy and pass.
 */
const CAPITALIZED_BRAND = /(?<![A-Za-z0-9_$-])Noura(?![A-Za-z0-9_-])/;

function sourceFiles(directory: string): string[] {
	return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const path = join(directory, entry.name);
		if (entry.isDirectory()) return sourceFiles(path);
		const source =
			(entry.name.endsWith('.svelte') || entry.name.endsWith('.ts')) &&
			!entry.name.includes('.test.');
		return source ? [path] : [];
	});
}

describe('brand spelling', () => {
	test('app copy writes the brand in lowercase', () => {
		const root = import.meta.dir;
		const offenders = sourceFiles(root).flatMap((path) =>
			readFileSync(path, 'utf8')
				.split('\n')
				.flatMap((line, index) =>
					CAPITALIZED_BRAND.test(line)
						? [`${relative(root, path)}:${index + 1}: ${line.trim()}`]
						: [],
				),
		);
		expect(offenders).toEqual([]);
	});

	test('identifiers and header names are not flagged', () => {
		for (const text of [
			'getNouraClient()',
			'NOURA_BASE_INSTRUCTIONS',
			"'Noura-Blob-Complete'",
		])
			expect(CAPITALIZED_BRAND.test(text)).toBe(false);
		expect(CAPITALIZED_BRAND.test('Open Noura')).toBe(true);
		expect(CAPITALIZED_BRAND.test('Noura could not save')).toBe(true);
	});
});
