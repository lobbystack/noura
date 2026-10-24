// Packages a plugin for release: a zip archive holding the manifest and
// the component, which Noura installs. Noura compiles the component itself
// on each device, so the archive holds no compiled code.
import { readFile, writeFile } from 'node:fs/promises';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { zipSync } from 'fflate';

import { type Built, build } from './build.js';
import { MANIFEST_FILE } from './manifest.js';

export interface Packed extends Built {
	/** The archive's path. */
	archive: string;
}

/** The archive's file name: the plugin's ID and version. */
export function archiveName(id: string, version: string): string {
	return `${id}-${version}.zip`;
}

/**
 * Build the plugin in `folder`, then write its release archive next to
 * the component: `plugin.toml` at the archive's root and the component at
 * the path the manifest's `entry` names.
 */
export async function pack(folder: string): Promise<Packed> {
	folder = resolve(folder);
	const built = await build(folder);
	const { manifest } = built;
	const entry = relative(folder, built.component).split(sep).join('/');
	const archive = join(
		dirname(built.component),
		archiveName(manifest.id, manifest.version),
	);
	const bytes = zipSync(
		{
			[MANIFEST_FILE]: await readFile(join(folder, MANIFEST_FILE)),
			[entry]: await readFile(built.component),
		},
		{ level: 9, mtime: new Date('2026-01-01T00:00:00Z') },
	);
	await writeFile(archive, bytes);
	return { ...built, archive };
}
