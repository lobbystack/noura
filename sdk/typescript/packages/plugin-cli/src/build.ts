// Builds a plugin's `src/plugin.ts` into the WebAssembly component its
// manifest names as `entry`, against the WIT of the interface version the
// manifest asks for.
import { spawn } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
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
 * leaves one out gets: nothing to show, and no event handled.
 */
const DEFAULTS = `
const nothing = () => [];
const ignore = () => {};
const none = () => undefined;
const part = (exported, defaults) =>
	Object.fromEntries(
		Object.entries(defaults).map(([name, fallback]) => [
			name,
			typeof exported?.[name] === 'function' ? exported[name].bind(exported) : fallback,
		]),
	);
export const view = part(plugin.view, {
	pages: nothing,
	render: nothing,
	sidebar: nothing,
	rows: nothing,
	handleEvent: ignore,
});
export const objectTypes = part(plugin.objectTypes, {
	types: nothing,
	renderPanel: nothing,
	handlePanelEvent: ignore,
});
export const commands = part(plugin.commands, {
	commands: nothing,
	run: (id) => {
		throw \`The plugin has no command \${id}.\`;
	},
});
export const inbox = part(plugin.inbox, {
	sections: nothing,
	renderSection: nothing,
	handleSectionEvent: ignore,
});
export const settings = part(plugin.settings, {
	page: none,
	renderSettings: nothing,
	handleSettingsEvent: ignore,
});
export const tools = part(plugin.tools, {
	tools: nothing,
	callTool: (name) => {
		throw \`The plugin has no tool \${name}.\`;
	},
});
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
	const component = resolve(folder, manifest.entry);
	if (!component.startsWith(folder + sep)) {
		throw new BuildError(`The entry ${manifest.entry} in plugin.toml isn't inside the plugin's folder.`);
	}
	const work = await mkdtemp(join(tmpdir(), 'noura-plugin-'));
	try {
		const entry = join(work, 'entry.ts');
		const imported = source.split(sep).join('/');
		await writeFile(
			entry,
			`// @ts-nocheck\nimport * as plugin from ${JSON.stringify(imported)};\n${DEFAULTS}`,
		);
		const out = join(work, 'plugin.wasm');
		await componentize(entry, wit, out, folder);
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

async function exists(path: string): Promise<boolean> {
	try {
		await access(path);
		return true;
	} catch {
		return false;
	}
}

/** Turn the module `entry` into a component for the `plugin` world. */
function componentize(entry: string, wit: string, out: string, cwd: string): Promise<void> {
	const jco = join(dirname(createRequire(import.meta.url).resolve('@bytecodealliance/jco')), 'jco.js');
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
		'--out',
		out,
	];
	return new Promise((done, fail) => {
		const child = spawn(process.execPath, args, {
			cwd,
			stdio: ['ignore', 'pipe', 'pipe'],
			env: { ...process.env, NO_COLOR: '1', FORCE_COLOR: '0' },
		});
		let output = '';
		child.stdout.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.stderr.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.on('error', (error) => fail(new BuildError(`The build didn't start: ${plain(error.message)}`)));
		child.on('close', (code) => {
			if (code === 0) {
				done();
			} else {
				fail(new BuildError(`The build failed.\n${plain(output.trim())}`));
			}
		});
	});
}
