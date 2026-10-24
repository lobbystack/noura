#!/usr/bin/env node
// noura-plugin: builds, rebuilds on save and packages a Noura plugin.
import { readFileSync } from 'node:fs';
import { relative, resolve, sep } from 'node:path';

import { BuildError, build, describe } from './build.js';
import { dev } from './dev.js';
import { ManifestError } from './manifest.js';
import { plain } from './output.js';
import { pack } from './pack.js';

const USAGE = `Usage: noura-plugin <command> [folder]

Commands:
  build     Build src/plugin.ts into the component plugin.toml names.
  dev       Build, then build again each time you save. Install the
            folder in Noura from Settings > Plugins > Install from folder,
            and Noura reloads the plugin after each build.
  package   Build, then write the release archive: plugin.toml and the
            component in one zip file.

The folder defaults to the current one.`;

function version(): string {
	const manifest = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as {
		version: string;
	};
	return manifest.version;
}

async function main(args: string[]): Promise<number> {
	const [command, folderArgument, ...rest] = args;
	if (command === '--version' || command === '-v') {
		console.log(version());
		return 0;
	}
	if (!command || command === '--help' || command === '-h' || command === 'help') {
		console.log(USAGE);
		return command ? 0 : 1;
	}
	if (rest.length > 0) {
		console.error(`noura-plugin ${command} takes one folder.\n\n${USAGE}`);
		return 1;
	}
	const folder = resolve(folderArgument ?? '.');
	switch (command) {
		case 'build':
			console.log(describe(await build(folder), folder));
			return 0;
		case 'package': {
			const packed = await pack(folder);
			const archive = relative(folder, packed.archive).split(sep).join('/');
			console.log(`${describe(packed, folder)}\nWrote the release archive ${archive}.`);
			return 0;
		}
		case 'dev': {
			const session = await dev(folder, (line) => console.log(line));
			await new Promise<void>((stop) => {
				process.once('SIGINT', stop);
				process.once('SIGTERM', stop);
			});
			await session.close();
			return 0;
		}
		default:
			console.error(`noura-plugin doesn't have a ${command} command.\n\n${USAGE}`);
			return 1;
	}
}

main(process.argv.slice(2)).then(
	(code) => process.exit(code),
	(error: unknown) => {
		if (error instanceof BuildError || error instanceof ManifestError) {
			console.error(error.message);
		} else {
			console.error(plain(String((error as Error)?.stack ?? error)));
		}
		process.exit(1);
	},
);
