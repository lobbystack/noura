// Runs the kit's command-line tools from this workspace's builds, as an
// author's project runs them from its node_modules.
import { spawn } from 'node:child_process';
import { cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const KIT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

/** The command-line tools the kit's packages offer. */
export const TOOLS = {
	'create-noura-plugin': join(KIT, 'packages/create-plugin/dist/cli.js'),
	'noura-plugin': join(KIT, 'packages/plugin-cli/dist/cli.js'),
} as const;

export interface Ran {
	code: number | null;
	/** Standard output and standard error, in the order they came. */
	output: string;
}

/** Run one of the kit's tools in `cwd` and wait for it to stop. */
export function run(tool: keyof typeof TOOLS, args: string[], cwd: string): Promise<Ran> {
	return new Promise((done, fail) => {
		const child = spawn(process.execPath, [TOOLS[tool], ...args], { cwd });
		let output = '';
		child.stdout.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.stderr.on('data', (chunk: Buffer) => (output += chunk.toString()));
		child.on('error', fail);
		child.on('close', (code) => done({ code, output }));
	});
}

/** A tool that keeps running, such as dev mode. */
export interface Running {
	/** Everything it printed so far. */
	output(): string;
	/** Wait until it printed `count` lines that match `pattern`. */
	waitFor(pattern: RegExp, count?: number): Promise<void>;
	stop(): Promise<void>;
}

/** Start one of the kit's tools in `cwd`, leaving it running. */
export function start(tool: keyof typeof TOOLS, args: string[], cwd: string): Running {
	const child = spawn(process.execPath, [TOOLS[tool], ...args], { cwd });
	let output = '';
	const listeners = new Set<() => void>();
	const heard = (chunk: Buffer) => {
		output += chunk.toString();
		listeners.forEach((listener) => listener());
	};
	child.stdout.on('data', heard);
	child.stderr.on('data', heard);
	const exited = new Promise<void>((stopped) => child.once('exit', () => stopped()));
	return {
		output: () => output,
		waitFor(pattern, count = 1) {
			const matches = () => output.split('\n').filter((line) => pattern.test(line)).length >= count;
			return new Promise((done, fail) => {
				const check = () => {
					if (matches()) {
						listeners.delete(check);
						done();
					} else if (child.exitCode !== null) {
						listeners.delete(check);
						fail(new Error(`${tool} stopped with ${child.exitCode}:\n${output}`));
					}
				};
				listeners.add(check);
				child.once('exit', check);
				check();
			});
		},
		async stop() {
			if (child.exitCode === null) {
				child.kill();
			}
			await exited;
		},
	};
}

/** Run a tool and fail with its output unless it succeeds. */
export async function succeed(tool: keyof typeof TOOLS, args: string[], cwd: string): Promise<string> {
	const ran = await run(tool, args, cwd);
	if (ran.code !== 0) {
		throw new Error(`${tool} ${args.join(' ')} stopped with ${ran.code}:\n${ran.output}`);
	}
	return ran.output;
}

/**
 * Copy the test plugin `name` from `tests/plugins` into `folder` and build
 * it with the CLI. `capabilities`, when given, replaces the ones its
 * manifest declares, each written as its `[[capabilities]]` table's body,
 * such as `name = "views"`.
 */
export async function buildTestPlugin(name: string, folder: string, capabilities?: string[]): Promise<string> {
	const project = join(folder, name);
	await cp(join(KIT, 'tests/plugins', name), project, { recursive: true });
	if (capabilities) {
		const manifest = await readFile(join(project, 'plugin.toml'), 'utf8');
		const head = manifest.split('[[capabilities]]')[0]?.trimEnd() ?? '';
		const tables = capabilities.map((capability) => `[[capabilities]]\n${capability}\n`).join('\n');
		await writeFile(join(project, 'plugin.toml'), `${head}\n\n${tables}`);
	}
	await succeed('noura-plugin', ['build'], project);
	return project;
}

/** A folder that `use` gets and that is removed afterwards. */
export async function inTemporaryFolder<T>(use: (folder: string) => Promise<T>): Promise<T> {
	const folder = await mkdtemp(join(tmpdir(), 'noura-kit-'));
	try {
		return await use(folder);
	} finally {
		await rm(folder, { recursive: true, force: true });
	}
}
