// The Node versions the kit's packages declare, which every package they
// and a new project install must accept, and which the guide names.
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { KIT } from './support/kit.js';

const semver = createRequire(import.meta.url)('semver') as {
	subset(range: string, of: string): boolean;
};

const PACKAGES = ['create-plugin', 'mock-host', 'plugin-cli', 'plugin-types'];

/** Packages only this repository's checks run, on CI's Node, which no author installs. */
const KIT_TOOLS = new Set(['node_modules/license-checker-rseidelsohn']);

/** A package's manifest, or an entry of the lockfile, for its engines. */
interface Package {
	engines?: { node?: string };
}

async function json<T>(path: string): Promise<T> {
	return JSON.parse(await readFile(path, 'utf8')) as T;
}

async function declared(): Promise<string> {
	const kit = await json<Package>(join(KIT, 'package.json'));
	const range = kit.engines?.node;
	if (!range) {
		throw new Error("The kit's package.json declares no Node versions.");
	}
	return range;
}

test('each @noura package declares the Node versions the kit does', async () => {
	const range = await declared();
	for (const name of PACKAGES) {
		const pkg = await json<Package>(
			join(KIT, 'packages', name, 'package.json'),
		);
		expect(pkg.engines?.node, name).toBe(range);
	}
});

test('every package the kit installs accepts those Node versions', async () => {
	const range = await declared();
	const lock = await json<{ packages: Record<string, Package> }>(
		join(KIT, 'package-lock.json'),
	);
	const narrower = Object.entries(lock.packages)
		.filter(([path, entry]) => {
			const node = entry.engines?.node;
			return (
				path.startsWith('node_modules/') &&
				!KIT_TOOLS.has(path) &&
				typeof node === 'string' &&
				!semver.subset(range, node)
			);
		})
		.map(([path, entry]) => `${path} needs Node ${entry.engines?.node}`);
	expect(narrower).toEqual([]);
});

test('the guide names those Node versions', async () => {
	const guide = await readFile(
		join(KIT, '../../docs/plugins/README.md'),
		'utf8',
	);
	expect(guide).toContain(
		'The kit needs Node 22.20 or a later 22 release, 24.12 or a later 24 release, or 26 or later.',
	);
	expect(await declared()).toBe('^22.20.0 || ^24.12.0 || >=26');
});
