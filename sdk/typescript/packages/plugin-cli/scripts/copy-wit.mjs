// Copies the WIT of each interface version the CLI builds plugins against
// into the package's wit/ folder: plugin-sdk's WIT, the one definition of
// Noura's plugin interface, under its current version, and the frozen copy
// of each earlier version from wit-frozen/, so plugins built against an
// earlier version keep building.
//
// `--freeze` writes plugin-sdk's WIT into wit-frozen/ under its version
// instead. The kit's tests fail while a frozen copy of the current version
// differs from plugin-sdk's WIT.
import { cpSync, existsSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const wit = join(here, '../../../../../crates/plugin-sdk/wit');
const frozen = join(here, '../wit-frozen');
const version = /^package noura:plugin@([0-9.]+);/m.exec(
	readFileSync(join(wit, 'plugin.wit'), 'utf8'),
)?.[1];
if (!version) {
	throw new Error(`${wit}/plugin.wit names no noura:plugin version.`);
}
if (process.argv.includes('--freeze')) {
	rmSync(join(frozen, version), { recursive: true, force: true });
	cpSync(wit, join(frozen, version), { recursive: true });
} else {
	const out = join(here, '../wit');
	rmSync(out, { recursive: true, force: true });
	if (existsSync(frozen)) {
		for (const earlier of readdirSync(frozen)) {
			if (earlier !== version) {
				cpSync(join(frozen, earlier), join(out, earlier), { recursive: true });
			}
		}
	}
	cpSync(wit, join(out, version), { recursive: true });
}
