import { resolve } from 'node:path';

const packageRoot = resolve(import.meta.dir, '..');
const repositoryRoot = resolve(packageRoot, '../..');
const wasmPath = resolve(
	repositoryRoot,
	'target/wasm32-unknown-unknown/release/workspace_format_wasm.wasm',
);
const outputDirectory = resolve(packageRoot, 'wasm');

async function run(command: string[], cwd: string) {
	const process = Bun.spawn(command, {
		cwd,
		stderr: 'inherit',
		stdout: 'inherit',
	});
	if ((await process.exited) !== 0) {
		throw new Error(`Command failed: ${command.join(' ')}`);
	}
}

await run(
	[
		'cargo',
		'build',
		'--release',
		'--target',
		'wasm32-unknown-unknown',
		'--package',
		'workspace-format-wasm',
	],
	repositoryRoot,
);

const wasmBindgen = Bun.which('wasm-bindgen');
if (!wasmBindgen) {
	throw new Error(
		'wasm-bindgen CLI is required to generate browser bindings. Install it with `cargo install wasm-bindgen-cli --version 0.2.127`.',
	);
}

await run(
	[
		wasmBindgen,
		wasmPath,
		'--target',
		'web',
		'--out-dir',
		outputDirectory,
		'--out-name',
		'workspace_format_wasm',
		'--typescript',
	],
	packageRoot,
);
