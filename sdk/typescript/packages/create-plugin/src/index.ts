// Writes a new plugin project from the template: the manifest, the
// example, its test, and the package and TypeScript setup that build and
// test it with the kit.
import { readFileSync } from 'node:fs';
import { cp, mkdir, readFile, readdir, rename, stat, writeFile } from 'node:fs/promises';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const TEMPLATE = fileURLToPath(new URL('../template/', import.meta.url));

/** The kit's version, which every `@noura` package shares. */
const KIT_VERSION = (
	JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as { version: string }
).version;

/** The versions of the tools a new project checks and tests with. */
const TYPESCRIPT = '7.0.2';
const VITEST = '5.0.5';

/** A project that can't be written, with a message for the author. */
export class CreateError extends Error {
	override name = 'CreateError';
}

export interface Created {
	/** The project's folder. */
	folder: string;
	/** The plugin's ID, which is the folder's name. */
	id: string;
	/** The plugin's name, as Noura shows it. */
	name: string;
}

/** Whether `id` can be a plugin's ID, as Noura's manifest rules say. */
export function validId(id: string): boolean {
	return /^[a-z][a-z0-9-]{0,63}$/.test(id);
}

/** The name Noura shows for the ID `id`: `word-count` is `Word count`. */
export function nameFor(id: string): string {
	const words = id.replace(/-+/g, ' ').trim();
	return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Write a new plugin project into `folder`, which must not exist yet. */
export async function createPlugin(folder: string): Promise<Created> {
	folder = resolve(folder);
	const id = basename(folder);
	if (!validId(id)) {
		throw new CreateError(
			`${id} can't name a plugin. Use lowercase letters, digits and hyphens, starting with a letter, such as word-count.`,
		);
	}
	if (await exists(folder)) {
		throw new CreateError(`${folder} exists already. Choose a new folder for the plugin.`);
	}
	const name = nameFor(id);
	await mkdir(folder, { recursive: true });
	await cp(TEMPLATE, folder, { recursive: true });
	await rename(join(folder, 'gitignore'), join(folder, '.gitignore'));
	for (const file of await files(folder)) {
		const text = await readFile(file, 'utf8');
		const filled = text.replaceAll('{{id}}', id).replaceAll('{{name}}', name);
		if (filled !== text) {
			await writeFile(file, filled);
		}
	}
	await writeFile(join(folder, 'package.json'), `${JSON.stringify(packageJson(id), null, '\t')}\n`);
	return { folder, id, name };
}

function packageJson(id: string) {
	const kit = `^${KIT_VERSION}`;
	return {
		name: id,
		version: '0.1.0',
		private: true,
		type: 'module',
		scripts: {
			build: 'noura-plugin build',
			dev: 'noura-plugin dev',
			package: 'noura-plugin package',
			check: 'tsc -p .',
			test: 'noura-plugin build && vitest run',
		},
		devDependencies: {
			'@noura/mock-host': kit,
			'@noura/plugin-cli': kit,
			'@noura/plugin-types': kit,
			typescript: TYPESCRIPT,
			vitest: VITEST,
		},
	};
}

async function exists(path: string): Promise<boolean> {
	try {
		await stat(path);
		return true;
	} catch {
		return false;
	}
}

/** Every file under `folder`. */
async function files(folder: string): Promise<string[]> {
	const found: string[] = [];
	for (const entry of await readdir(folder, { withFileTypes: true })) {
		const path = join(folder, entry.name);
		if (entry.isDirectory()) {
			found.push(...(await files(path)));
		} else {
			found.push(path);
		}
	}
	return found;
}
