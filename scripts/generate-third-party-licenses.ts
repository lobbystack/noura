/**
 * Writes the third-party license file bundled with the desktop app.
 *
 * - JavaScript: the packages Vite rendered into the desktop frontend, read from
 *   `apps/app/.svelte-kit/client-inputs.json`. Run `bun run --cwd apps/app build`
 *   first.
 * - Rust: the crates `noura-desktop` links through normal dependencies, from
 *   `cargo metadata` and Cargo.lock.
 *
 * Identical license texts appear once, followed by the packages that use them.
 *
 * Usage:
 *   bun scripts/generate-third-party-licenses.ts --out <file> [--target <triple>]...
 */
import { createHash } from 'node:crypto';
import { readdir, readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

const root = resolve(import.meta.dir, '..');
const args = process.argv.slice(2);
const targets: string[] = [];
let out = '';
for (let index = 0; index < args.length; index += 1) {
	if (args[index] === '--out') out = args[++index] ?? '';
	else if (args[index] === '--target') targets.push(args[++index] ?? '');
	else throw new Error(`Unknown argument: ${args[index]}`);
}
if (!out) throw new Error('Pass --out <file>');

interface Component {
	ecosystem: 'npm' | 'crate';
	name: string;
	version: string;
	license: string;
	texts: string[];
}

const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)([._-]|$)/i;

async function licenseTexts(directory: string): Promise<string[]> {
	const texts: string[] = [];
	for (const name of (await readdir(directory)).sort()) {
		if (!LICENSE_FILE.test(name)) continue;
		const path = join(directory, name);
		if ((await stat(path)).isFile()) texts.push(await readFile(path, 'utf8'));
	}
	return texts;
}

async function javascriptComponents(): Promise<Component[]> {
	const manifest = Bun.file(
		join(root, 'apps/app/.svelte-kit/client-inputs.json'),
	);
	if (!(await manifest.exists()))
		throw new Error('Build apps/app first: client-inputs.json is missing');
	const { version, inputs } = (await manifest.json()) as {
		version: number;
		inputs: string[];
	};
	if (version !== 1) throw new Error('Unsupported client-inputs.json version');

	const packages = new Map<string, Component>();
	for (const input of inputs) {
		let directory = dirname(await realpath(input));
		while (directory !== dirname(directory)) {
			const file = Bun.file(join(directory, 'package.json'));
			if (await file.exists()) {
				const pkg = (await file.json()) as {
					name?: string;
					version?: string;
					license?: string;
				};
				if (pkg.name && pkg.version) {
					const key = `${pkg.name}@${pkg.version}`;
					if (!packages.has(key)) {
						let texts = await licenseTexts(directory);
						if (texts.length === 0) {
							// Some packages publish without a license file. The repository
							// keeps the upstream text for those next to the server build.
							const fallback = Bun.file(
								join(
									root,
									'apps/server/licenses',
									`${pkg.name.replace(/^@/, '').replace('/', '-')}-${pkg.version}.txt`,
								),
							);
							if (await fallback.exists()) texts = [await fallback.text()];
						}
						packages.set(key, {
							ecosystem: 'npm',
							name: pkg.name,
							version: pkg.version,
							license: pkg.license ?? 'see license text',
							texts,
						});
					}
					break;
				}
			}
			directory = dirname(directory);
		}
	}
	return [...packages.values()];
}

interface CargoMetadata {
	packages: Array<{
		id: string;
		name: string;
		version: string;
		license: string | null;
		license_file: string | null;
		manifest_path: string;
		source: string | null;
	}>;
	resolve: {
		nodes: Array<{
			id: string;
			deps: Array<{ pkg: string; dep_kinds: Array<{ kind: string | null }> }>;
		}>;
	};
}

async function rustComponents(): Promise<Component[]> {
	const command = [
		'cargo',
		'metadata',
		'--format-version',
		'1',
		'--locked',
		...targets.flatMap((target) => ['--filter-platform', target]),
	];
	const result = Bun.spawnSync(command, { cwd: root, stderr: 'inherit' });
	if (result.exitCode !== 0) throw new Error('cargo metadata failed');
	const metadata = JSON.parse(result.stdout.toString()) as CargoMetadata;

	const byId = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
	const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
	const desktop = metadata.packages.find((pkg) => pkg.name === 'noura-desktop');
	if (!desktop) throw new Error('noura-desktop is missing from cargo metadata');

	// Follow normal dependencies only. Build and dev dependencies don't ship.
	const linked = new Set<string>();
	const queue = [desktop.id];
	while (queue.length > 0) {
		const id = queue.pop()!;
		if (linked.has(id)) continue;
		linked.add(id);
		for (const dep of nodes.get(id)?.deps ?? []) {
			if (dep.dep_kinds.some((kind) => kind.kind === null)) queue.push(dep.pkg);
		}
	}

	const components: Component[] = [];
	for (const id of linked) {
		const pkg = byId.get(id);
		// Path dependencies are noura's own crates, covered by its MIT license.
		if (!pkg || pkg.source === null) continue;
		const directory = dirname(pkg.manifest_path);
		const texts = await licenseTexts(directory);
		if (pkg.license_file) {
			const path = resolve(directory, pkg.license_file);
			const text = await readFile(path, 'utf8').catch(() => null);
			if (text && !texts.includes(text)) texts.push(text);
		}
		components.push({
			ecosystem: 'crate',
			name: pkg.name,
			version: pkg.version,
			license: pkg.license ?? 'see license text',
			texts,
		});
	}
	return components;
}

const components = [
	...(await javascriptComponents()),
	...(await rustComponents()),
].sort(
	(a, b) =>
		a.ecosystem.localeCompare(b.ecosystem) ||
		a.name.localeCompare(b.name) ||
		a.version.localeCompare(b.version, undefined, { numeric: true }),
);

const label = (component: Component) =>
	`${component.name} ${component.version} (${component.ecosystem})`;
const textIds = new Map<
	string,
	{ id: number; text: string; users: string[] }
>();
const missing: string[] = [];
for (const component of components) {
	if (component.texts.length === 0) missing.push(label(component));
	for (const text of component.texts) {
		const normalized = text.replace(/\r\n/g, '\n').trim();
		const hash = createHash('sha256').update(normalized).digest('hex');
		const entry = textIds.get(hash) ?? {
			id: textIds.size + 1,
			text: normalized,
			users: [],
		};
		entry.users.push(label(component));
		textIds.set(hash, entry);
	}
}

const rule = '='.repeat(78);
const lines = [
	'THIRD-PARTY SOFTWARE LICENSES',
	'',
	'noura includes the open source packages listed below. Each keeps its own',
	'license. The full license texts follow the list.',
	'',
	`Generated from Cargo.lock and bun.lock on ${new Date().toISOString().slice(0, 10)}.`,
	'',
	rule,
	'PACKAGES',
	rule,
	'',
	...components.map((component) => `${label(component)}: ${component.license}`),
	'',
];
if (missing.length > 0) {
	lines.push(
		rule,
		'PACKAGES THAT SHIP NO LICENSE FILE',
		rule,
		'',
		'These packages declare the license shown in the list above but include no',
		'license file of their own.',
		'',
		...missing,
		'',
	);
}
for (const entry of textIds.values()) {
	lines.push(
		rule,
		`LICENSE TEXT ${entry.id}`,
		`Used by: ${entry.users.join(', ')}`,
		rule,
		'',
		entry.text,
		'',
	);
}

await writeFile(resolve(out), lines.join('\n'));
console.info(
	`Wrote ${components.length} packages and ${textIds.size} license texts to ${out}` +
		(missing.length > 0 ? ` (${missing.length} without a license file)` : ''),
);
