// Loads a plugin's component in Node: jco turns it into JavaScript and
// core WebAssembly modules, which the mock host instantiates with its own
// implementation of each capability interface.
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { transpile } from '@bytecodealliance/jco';

/** What a component's interfaces offer once instantiated, by interface. */
export type Exports = Record<
	string,
	Record<string, (...args: unknown[]) => unknown>
>;

/** The functions the mock host gives the component, by interface name without its version. */
export type Imports = object;

/** A component turned into JavaScript, ready to instantiate. */
export interface Loaded {
	instantiate(imports: Imports): Promise<Exports>;
	/** Remove the files jco wrote. */
	dispose(): Promise<void>;
}

/**
 * `imports`, with every other interface the component imports answered by
 * functions that fail when called, as Noura links the imports it doesn't
 * provide.
 */
function withTraps(imports: Imports): Imports {
	const trapping = (face: string) =>
		new Proxy(
			{},
			{
				get: (_, name) => () => {
					throw new Error(
						`The plugin called ${face}#${String(name)}, which Noura doesn't provide.`,
					);
				},
			},
		);
	return new Proxy(imports, {
		get: (target, face: string) =>
			(target as Record<string, object>)[face] ?? trapping(face),
	});
}

/** Turn the component `bytes` into JavaScript in a folder of its own. */
export async function load(bytes: Uint8Array): Promise<Loaded> {
	const { files } = await transpile(bytes, {
		name: 'plugin',
		instantiation: 'async',
		noTypescript: true,
		quiet: true,
	});
	const folder = await mkdtemp(join(tmpdir(), 'noura-mock-host-'));
	for (const [name, data] of Object.entries(files)) {
		if (name.endsWith('.d.ts')) {
			continue;
		}
		await mkdir(dirname(join(folder, name)), { recursive: true });
		await writeFile(join(folder, name), data);
	}
	const module = (await import(
		pathToFileURL(join(folder, 'plugin.js')).href
	)) as {
		instantiate(
			getCoreModule: (name: string) => Promise<WebAssembly.Module>,
			imports: Imports,
		): Promise<Exports>;
	};
	return {
		instantiate: (imports) =>
			module.instantiate(
				async (name) => WebAssembly.compile(await readFile(join(folder, name))),
				withTraps(imports),
			),
		dispose: () => rm(folder, { recursive: true, force: true }),
	};
}
