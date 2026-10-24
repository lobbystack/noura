#!/usr/bin/env node
// create-noura-plugin: writes a new plugin project.
import { relative } from 'node:path';

import { CreateError, createPlugin } from './index.js';

const USAGE = `Usage: create-noura-plugin <folder>

Writes a Noura plugin project into a new folder. The folder's name is the
plugin's ID: lowercase letters, digits and hyphens, such as word-count.`;

async function main(args: string[]): Promise<number> {
	const [folder, ...rest] = args;
	if (folder === '--help' || folder === '-h') {
		console.log(USAGE);
		return 0;
	}
	if (!folder || rest.length > 0) {
		console.error(USAGE);
		return 1;
	}
	const created = await createPlugin(folder);
	const shown = relative(process.cwd(), created.folder) || '.';
	console.log(`Created the plugin ${created.name} (${created.id}) in ${shown}.

Next:
  cd ${shown}
  npm install
  npm run dev

Then install the folder in Noura from Settings > Plugins > Install from
folder. Noura reloads the plugin each time dev mode builds it.`);
	return 0;
}

main(process.argv.slice(2)).then(
	(code) => process.exit(code),
	(error: unknown) => {
		console.error(error instanceof CreateError ? error.message : String((error as Error)?.stack ?? error));
		process.exit(1);
	},
);
