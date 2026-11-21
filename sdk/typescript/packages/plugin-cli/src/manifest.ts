// Reads a plugin's manifest, `plugin.toml`, for the fields the kit uses.
// Noura checks every rule of the manifest when it installs the plugin;
// this reads only what a build, a package and the mock host need.
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { parse } from 'smol-toml';

/** The manifest's file name inside a plugin's folder. */
export const MANIFEST_FILE = 'plugin.toml';

/** A capability the manifest declares, with its hosts or targets. */
export interface Capability {
	name: string;
	/** The hosts of the `network` capability. */
	hosts: string[];
	/** The targets of the `commands` capability. */
	targets: string[];
	/** The shortcuts of the `global-shortcuts` capability. */
	shortcuts: Shortcut[];
	/** The events of the `events` capability. */
	events: EventDeclaration[];
}

/** An event the `events` capability declares, which Orbs' triggers name
 * as `<plugin>.<id>`. */
export interface EventDeclaration {
	id: string;
	title: string;
}

/** A shortcut the `global-shortcuts` capability declares. */
export interface Shortcut {
	id: string;
	title: string;
	/** The keys it starts with on this system, such as `Alt+Space`:
	 * `macos-keys`, `windows-keys` or `linux-keys` when the manifest gives
	 * them, `keys` otherwise. */
	keys: string;
}

export interface Manifest {
	id: string;
	name: string;
	version: string;
	/** The `noura:plugin` version the plugin is built against. */
	interfaceVersion: string;
	/** Where the build writes the component, relative to the folder. */
	entry: string;
	capabilities: Capability[];
}

/** A manifest that is missing, isn't TOML or lacks a field. */
export class ManifestError extends Error {
	override name = 'ManifestError';
}

/** Read the manifest in the plugin folder `folder`. */
export async function readManifest(folder: string): Promise<Manifest> {
	const path = join(folder, MANIFEST_FILE);
	let text: string;
	try {
		text = await readFile(path, 'utf8');
	} catch {
		throw new ManifestError(
			`${path} isn't there. A plugin's folder holds its ${MANIFEST_FILE}.`,
		);
	}
	let raw: Record<string, unknown>;
	try {
		raw = parse(text);
	} catch (error) {
		throw new ManifestError(
			`${path} isn't valid TOML: ${(error as Error).message}`,
		);
	}
	const field = (name: string): string => {
		const value = raw[name];
		if (typeof value !== 'string' || value.trim() === '') {
			throw new ManifestError(`${path} needs ${name} as text.`);
		}
		return value;
	};
	const strings = (value: unknown): string[] =>
		Array.isArray(value)
			? value.filter((item): item is string => typeof item === 'string')
			: [];
	const capabilities = (
		Array.isArray(raw.capabilities) ? raw.capabilities : []
	).map((entry: unknown): Capability => {
		const capability = (entry ?? {}) as Record<string, unknown>;
		if (typeof capability.name !== 'string') {
			throw new ManifestError(`${path} lists a capability without a name.`);
		}
		const shortcuts = (
			Array.isArray(capability.shortcuts) ? capability.shortcuts : []
		).flatMap((shortcut: unknown): Shortcut[] => {
			const declared = (shortcut ?? {}) as Record<string, unknown>;
			if (
				typeof declared.id !== 'string' ||
				typeof declared.title !== 'string' ||
				typeof declared.keys !== 'string'
			) {
				return [];
			}
			// Keys for this system replace `keys`, as Noura does.
			const system =
				process.platform === 'darwin'
					? 'macos-keys'
					: process.platform === 'win32'
						? 'windows-keys'
						: 'linux-keys';
			const here = declared[system];
			return [
				{
					id: declared.id,
					title: declared.title,
					keys: typeof here === 'string' ? here : declared.keys,
				},
			];
		});
		const events = (
			Array.isArray(capability.events) ? capability.events : []
		).flatMap((event: unknown): EventDeclaration[] => {
			const declared = (event ?? {}) as Record<string, unknown>;
			return typeof declared.id === 'string' &&
				typeof declared.title === 'string'
				? [{ id: declared.id, title: declared.title }]
				: [];
		});
		return {
			name: capability.name,
			hosts: strings(capability.hosts),
			targets: strings(capability.targets),
			shortcuts,
			events,
		};
	});
	return {
		id: field('id'),
		name: field('name'),
		version: field('version'),
		interfaceVersion: field('interface-version'),
		entry: field('entry'),
		capabilities,
	};
}
