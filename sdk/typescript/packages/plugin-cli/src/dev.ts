// Dev mode: build the plugin, then build it again each time its source or
// manifest changes. Noura reloads a plugin installed from this folder when
// a new build lands in it.
import { join, resolve, sep } from 'node:path';
import { watch } from 'chokidar';

import { BuildError, build, describe } from './build.js';
import { MANIFEST_FILE, ManifestError, readManifest } from './manifest.js';
import { plain } from './output.js';

/** A dev session, which watches until closed. */
export interface Dev {
	close(): Promise<void>;
}

/** How long the source must stay quiet after a change before a build. */
const QUIET_MS = 100;

/** Why a build failed, in a line for the author. */
function failure(error: unknown): string {
	if (error instanceof BuildError || error instanceof ManifestError) {
		return error.message;
	}
	const message = error instanceof Error ? error.message : String(error);
	return `The build failed: ${plain(message)}`;
}

/**
 * Build the plugin in `folder` and again after each change to `src` or
 * its manifest. Each build reports to `log`, and a failed one says why
 * and leaves dev mode watching.
 */
export async function dev(
	folder: string,
	log: (line: string) => void,
): Promise<Dev> {
	folder = resolve(folder);
	const src = join(folder, 'src');
	let building = false;
	let again = false;
	let timer: NodeJS.Timeout | undefined;
	const run = async () => {
		if (building) {
			again = true;
			return;
		}
		building = true;
		try {
			const manifest = await readManifest(folder);
			const entry = resolve(folder, manifest.entry);
			if (entry === src || entry.startsWith(src + sep)) {
				// Writing the component there would start the next build.
				log(
					`The entry ${manifest.entry} in plugin.toml is inside src, which dev mode watches. Put the component outside src, such as at dist/plugin.wasm.`,
				);
			} else {
				log(describe(await build(folder), folder));
			}
		} catch (error) {
			log(failure(error));
		} finally {
			building = false;
		}
		if (again) {
			again = false;
			await run();
		}
	};
	await run();
	const watcher = watch([src, join(folder, MANIFEST_FILE)], {
		ignoreInitial: true,
		awaitWriteFinish: { stabilityThreshold: QUIET_MS, pollInterval: 20 },
	});
	watcher.on('all', () => {
		clearTimeout(timer);
		timer = setTimeout(() => void run(), QUIET_MS);
	});
	await new Promise<void>((ready) => watcher.once('ready', () => ready()));
	log('Waiting for changes to src and plugin.toml.');
	return {
		async close() {
			clearTimeout(timer);
			await watcher.close();
		},
	};
}
