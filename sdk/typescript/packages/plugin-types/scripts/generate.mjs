// Generates the package's declarations from plugin-sdk's WIT, the one
// definition of Noura's plugin interface, into a folder named after the
// interface version.
import { execFileSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const wit = join(here, '../../../../../crates/plugin-sdk/wit');
const version = /^package noura:plugin@([0-9.]+);/m.exec(
	readFileSync(join(wit, 'plugin.wit'), 'utf8'),
)?.[1];
if (!version) {
	throw new Error(`${wit}/plugin.wit names no noura:plugin version.`);
}
const out = join(here, '..', version);
rmSync(out, { recursive: true, force: true });
const jco = join(
	dirname(createRequire(import.meta.url).resolve('@bytecodealliance/jco')),
	'jco.js',
);
execFileSync(
	process.execPath,
	[
		jco,
		'guest-types',
		wit,
		'--world-name',
		'plugin',
		'--out-dir',
		out,
		'--quiet',
	],
	{ stdio: 'inherit' },
);
