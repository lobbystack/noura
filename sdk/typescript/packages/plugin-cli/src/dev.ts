// Dev mode: build the plugin, then build it again each time its source or
// manifest changes. Noura reloads a plugin installed from this folder when
// a new build lands in it.
import { join, resolve } from 'node:path';
import { watch } from 'chokidar';

import { BuildError, build, describe } from './build.js';
import { MANIFEST_FILE, ManifestError } from './manifest.js';

/** A dev session, which watches until closed. */
export interface Dev {
	close(): Promise<void>;
}

/** How long the source must stay quiet after a change before a build. */
const QUIET_MS = 100;

/**
 * Build the plugin in `folder` and again after each change to `src` or
 * its manifest. Each build reports to `log`, and a failed one says why.
 */
export async function dev(
	folder: string,
	log: (line: string) => void,
): Promise<Dev> {
	folder = resolve(folder);
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
			log(describe(await build(folder), folder));
		} catch (error) {
			if (!(error instanceof BuildError || error instanceof ManifestError)) {
				throw error;
			}
			log(error.message);
		} finally {
			building = false;
		}
		if (again) {
			again = false;
			await run();
		}
	};
	await run();
	const watcher = watch([join(folder, 'src'), join(folder, MANIFEST_FILE)], {
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
