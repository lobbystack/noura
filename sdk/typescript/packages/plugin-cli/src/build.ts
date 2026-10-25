// Builds a plugin's `src/plugin.ts` into the WebAssembly component its
// manifest names as `entry`, against the WIT of the interface version the
// manifest asks for.
import { spawn } from 'node:child_process';
import {
	access,
	mkdir,
	mkdtemp,
	open,
	readFile,
	rename,
	rm,
	stat,
	writeFile,
} from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { type Manifest, readManifest } from './manifest.js';
import { plain } from './output.js';

/** The plugin's source, relative to its folder. */
export const SOURCE = 'src/plugin.ts';

/** The WIT folders this kit holds, one per interface version. */
const WIT = fileURLToPath(new URL('../wit/', import.meta.url));

export interface Built {
	manifest: Manifest;
	/** The component's path. */
	component: string;
	/** How long the build took. */
	milliseconds: number;
}

/** A build that failed, with a message for the plugin's author. */
export class BuildError extends Error {
	override name = 'BuildError';
}

/**
 * The functions each exported interface needs, with what a plugin that
 * leaves one out gets: nothing to show, and no event handled. The
 * plugin's exports are looked up by name, so the bundler doesn't warn
 * about the interfaces a plugin leaves out.
 */
const DEFAULTS = `
const nothing = () => [];
const ignore = () => {};
const none = () => undefined;
const exported = (name) => plugin[name];
const part = (offered, defaults) =>
	Object.fromEntries(
		Object.entries(defaults).map(([name, fallback]) => [
			name,
			typeof offered?.[name] === 'function' ? offered[name].bind(offered) : fallback,
		]),
	);
export const view = part(exported('view'), {
	pages: nothing,
	render: nothing,
	sidebar: nothing,
	rows: nothing,
	handleEvent: ignore,
});
export const objectTypes = part(exported('objectTypes'), {
	types: nothing,
	renderPanel: nothing,
	handlePanelEvent: ignore,
});
export const commands = part(exported('commands'), {
	commands: nothing,
	run: (id) => {
		throw \`The plugin has no command \${id}.\`;
	},
});
export const inbox = part(exported('inbox'), {
	sections: nothing,
	renderSection: nothing,
	handleSectionEvent: ignore,
});
export const settings = part(exported('settings'), {
	page: none,
	renderSettings: nothing,
	handleSettingsEvent: ignore,
});
export const tools = part(exported('tools'), {
	tools: nothing,
	callTool: (name) => {
		throw \`The plugin has no tool \${name}.\`;
	},
});
`;

/** A console whose every method does nothing. */
const QUIET_CONSOLE = `const nothing = () => {};
export default new Proxy({}, { get: () => nothing });
`;

/** Build the plugin in `folder`. */
export async function build(folder: string): Promise<Built> {
	const started = performance.now();
	folder = resolve(folder);
	const manifest = await readManifest(folder);
	const source = join(folder, SOURCE);
	if (!(await exists(source))) {
		throw new BuildError(`${SOURCE} isn't there. The build starts from it.`);
	}
	const wit = join(WIT, manifest.interfaceVersion);
	if (!(await exists(join(wit, 'plugin.wit')))) {
		throw new BuildError(
			`plugin.toml asks for plugin interface ${manifest.interfaceVersion}, which this version of the kit doesn't build.`,
		);
	}
	const component = await componentPath(folder, manifest.entry);
	const work = await mkdtemp(join(tmpdir(), 'noura-plugin-'));
	try {
		const entry = join(work, 'entry.ts');
		const imported = source.split(sep).join('/');
		await writeFile(
			entry,
			`// @ts-nocheck\nimport * as plugin from ${JSON.stringify(imported)};\n${DEFAULTS}`,
		);
		// Noura gives plugins no console, so `console` is one that drops
		// what it's given rather than one the host would have to provide.
		const quiet = join(work, 'console.js');
		await writeFile(quiet, QUIET_CONSOLE);
		await writeFile(
			join(work, 'bundle.config.mjs'),
			`export default { transform: { inject: { console: [${JSON.stringify(quiet.split(sep).join('/'))}, 'default'] } } };\n`,
		);
		const out = join(work, 'plugin.wasm');
		await componentize(entry, wit, out, { work, folder });
		await mkdir(dirname(component), { recursive: true });
		// A rename within the folder, so a host watching it never reads half
		// a component.
		const staged = `${component}.${process.pid}.tmp`;
		await writeFile(staged, await readFile(out));
		await rename(staged, component);
	} finally {
		await rm(work, { recursive: true, force: true });
	}
	return { manifest, component, milliseconds: performance.now() - started };
}

/** What a build reports: the plugin, where it went and how long it took. */
export function describe(built: Built, folder: string): string {
	const where = relative(resolve(folder), built.component).split(sep).join('/');
	const seconds = (built.milliseconds / 1000).toFixed(1);
	return `Built ${built.manifest.name} ${built.manifest.version} into ${where} in ${seconds} s.`;
}

/** The first bytes of every WebAssembly module and component. */
const WASM_MAGIC = Buffer.from([0x00, 0x61, 0x73, 0x6d]);

/**
 * Where the build writes the component that `entry` names, once it's sure
 * the build can't write over the plugin's own files: the entry ends in
 * `.wasm`, lies inside the folder but outside `src`, and doesn't name a
 * file that holds something other than WebAssembly.
 */
async function componentPath(folder: string, entry: string): Promise<string> {
	const component = resolve(folder, entry);
	const named = `The entry ${entry} in plugin.toml`;
	if (!component.startsWith(folder + sep)) {
		throw new BuildError(`${named} isn't inside the plugin's folder.`);
	}
	if (!/\.wasm$/i.test(component)) {
		throw new BuildError(
			`${named} doesn't end in .wasm. It names the component the build writes, such as dist/plugin.wasm.`,
		);
	}
	const [top = ''] = relative(folder, component).split(sep);
	if (top.toLowerCase() === 'src') {
		throw new BuildError(
			`${named} is inside src, which holds the plugin's source. Put the component outside src, such as at dist/plugin.wasm.`,
		);
	}
	const head = await firstBytes(component, named);
	if (head && head.length > 0 && !head.equals(WASM_MAGIC)) {
		throw new BuildError(
			`${named} names a file that isn't a component, so the build won't write over it. Move that file, or name another path, such as dist/plugin.wasm.`,
		);
	}
	return component;
}

/** The first bytes of the file at `path`, or `undefined` when there's none. */
async function firstBytes(
	path: string,
	named: string,
): Promise<Buffer | undefined> {
	let found: Awaited<ReturnType<typeof stat>>;
	try {
		found = await stat(path);
	} catch (error) {
		const { code } = error as NodeJS.ErrnoException;
		if (code === 'ENOENT' || code === 'ENOTDIR') {
			return undefined;
		}
		throw error;
	}
	if (found.isDirectory()) {
		throw new BuildError(`${named} names a folder.`);
	}
	const file = await open(path, 'r');
	try {
		const head = Buffer.alloc(WASM_MAGIC.length);
		const { bytesRead } = await file.read(head, 0, head.length, 0);
		return head.subarray(0, bytesRead);
	} finally {
		await file.close();
	}
}

async function exists(path: string): Promise<boolean> {
	try {
		await access(path);
		return true;
	} catch {
		return false;
	}
}

/**
 * Turn the module `entry` into a component for the `plugin` world. `work`
 * is the folder that holds `entry`, and `folder` the plugin's.
 */
function componentize(
	entry: string,
	wit: string,
	out: string,
	{ work, folder }: { work: string; folder: string },
): Promise<void> {
	const jco = join(
		dirname(createRequire(import.meta.url).resolve('@bytecodealliance/jco')),
		'jco.js',
	);
	const args = [
		jco,
		'componentize',
		entry,
		'--wit',
		wit,
		'--world-name',
		'plugin',
		'--backend',
		'qjs',
		'--backend-qjs-disable-async',
		'--backend-qjs-stub-wasi',
		'--bundle-config',
		join(work, 'bundle.config.mjs'),
		'--out',
		out,
	];
	return new Promise((done, fail) => {
		const child = spawn(process.execPath, args, {
			cwd: folder,
			stdio: ['ignore', 'pipe', 'pipe'],
			env: { ...process.env, NO_COLOR: '1', FORCE_COLOR: '0' },
		});
		let output = '';
		child.stdout.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.stderr.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.on('error', (error) =>
			fail(new BuildError(`The build didn't start: ${plain(error.message)}`)),
		);
		child.on('close', (code) => {
			if (code === 0) {
				done();
			} else {
				fail(
					new BuildError(`The build failed.\n${failure(output, work, folder)}`),
				);
			}
		});
	});
}

/** Warnings the bundler gives for every plugin, which say nothing about a failure. */
const NOISE = ['[UNRESOLVED_IMPORT]', '[IMPORT_IS_UNDEFINED]'];

/** The escape sequences that color terminal output. */
const COLORS = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, 'g');

/**
 * What a failed build printed, for the plugin's author: without colors,
 * stack traces and the warnings every build gives, with paths relative to
 * the plugin's folder, and without the engine's name.
 */
function failure(output: string, work: string, folder: string): string {
	const text = output
		.replace(COLORS, '')
		.replace(/\(jco componentize\) /g, '')
		.replace(/(?:\.\.\/)+[^\s:\]]+/g, (path) => {
			const inFolder = relative(folder, resolve(work, path))
				.split(sep)
				.join('/');
			return inFolder.startsWith('..') ? path : inFolder;
		});
	const blocks = text
		.split(/\n\s*\n/)
		.filter((block) => !NOISE.some((noise) => block.includes(noise)))
		.map((block) =>
			block
				.split('\n')
				.filter(
					(line) => !/^\s+at |^\s*errors: \[Getter\/Setter\]|^\s*}$/.test(line),
				)
				.join('\n')
				.trimEnd(),
		)
		.filter((block) => block.trim() !== '');
	return plain(blocks.join('\n\n'));
}
