// Runs the kit's command-line tools from this workspace's builds, as an
// author's project runs them from its node_modules.
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
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

/** Run a tool and fail with its output unless it succeeds. */
export async function succeed(tool: keyof typeof TOOLS, args: string[], cwd: string): Promise<string> {
	const ran = await run(tool, args, cwd);
	if (ran.code !== 0) {
		throw new Error(`${tool} ${args.join(' ')} stopped with ${ran.code}:\n${ran.output}`);
	}
	return ran.output;
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
