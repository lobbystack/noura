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
		throw new ManifestError(`${path} isn't there. A plugin's folder holds its ${MANIFEST_FILE}.`);
	}
	let raw: Record<string, unknown>;
	try {
		raw = parse(text);
	} catch (error) {
		throw new ManifestError(`${path} isn't valid TOML: ${(error as Error).message}`);
	}
	const field = (name: string): string => {
		const value = raw[name];
		if (typeof value !== 'string' || value.trim() === '') {
			throw new ManifestError(`${path} needs ${name} as text.`);
		}
		return value;
	};
	const strings = (value: unknown): string[] =>
		Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
	const capabilities = (Array.isArray(raw.capabilities) ? raw.capabilities : []).map(
		(entry: unknown): Capability => {
			const capability = (entry ?? {}) as Record<string, unknown>;
			if (typeof capability.name !== 'string') {
				throw new ManifestError(`${path} lists a capability without a name.`);
			}
			return {
				name: capability.name,
				hosts: strings(capability.hosts),
				targets: strings(capability.targets),
			};
		},
	);
	return {
		id: field('id'),
		name: field('name'),
		version: field('version'),
		interfaceVersion: field('interface-version'),
		entry: field('entry'),
		capabilities,
	};
}
