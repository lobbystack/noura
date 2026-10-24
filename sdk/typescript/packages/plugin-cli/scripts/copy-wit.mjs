// Copies plugin-sdk's WIT, the one definition of Noura's plugin interface,
// into the package under its interface version, which the CLI builds
// plugins against.
import { cpSync, readFileSync, rmSync } from 'node:fs';
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
const out = join(here, '../wit');
rmSync(out, { recursive: true, force: true });
cpSync(wit, join(out, version), { recursive: true });
