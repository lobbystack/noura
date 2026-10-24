// Packs every @noura package, starts a fresh project with the packed
// create-noura-plugin, installs the packed archives into it, and checks,
// builds, tests and packages the template from them, as an author would.
import { execFileSync } from 'node:child_process';
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	rmSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const kit = join(dirname(fileURLToPath(import.meta.url)), '..');
const work = mkdtempSync(join(tmpdir(), 'noura-packed-'));
const packs = join(work, 'packs');
mkdirSync(packs);

function npm(args, cwd) {
	console.log(`\n$ npm ${args.join(' ')}`);
	execFileSync('npm', args, {
		cwd,
		stdio: 'inherit',
		shell: process.platform === 'win32',
	});
}

try {
	npm(['pack', '--workspaces', '--pack-destination', packs], kit);
	const archives = readdirSync(packs)
		.filter((name) => name.startsWith('noura-') && name.endsWith('.tgz'))
		.map((name) => join(packs, name));
	const expected = ['create-plugin', 'mock-host', 'plugin-cli', 'plugin-types'];
	for (const name of expected) {
		if (!archives.some((archive) => archive.includes(`noura-${name}-`))) {
			throw new Error(`npm pack wrote no archive for @noura/${name}.`);
		}
	}
	const create = archives.find((archive) =>
		archive.includes('noura-create-plugin-'),
	);
	npm(
		[
			'exec',
			'--yes',
			`--package=${create}`,
			'--',
			'create-noura-plugin',
			'word-count',
		],
		work,
	);
	const project = join(work, 'word-count');
	npm(
		[
			'install',
			'--no-audit',
			'--no-fund',
			...archives.filter((archive) => archive !== create),
		],
		project,
	);
	npm(['run', 'check'], project);
	npm(['test'], project);
	npm(['run', 'package'], project);
	for (const file of ['dist/plugin.wasm', 'dist/word-count-0.1.0.zip']) {
		if (!existsSync(join(project, file))) {
			throw new Error(`The packed kit didn't write ${file}.`);
		}
	}
	console.log('\nThe packed kit built, tested and packaged the template.');
} finally {
	rmSync(work, { recursive: true, force: true });
}
