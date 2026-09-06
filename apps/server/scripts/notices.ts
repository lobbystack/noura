import { dirname, join, resolve } from 'node:path';
import { readdir, readFile, realpath, stat, writeFile } from 'node:fs/promises';

// Inspect only packages actually compiled into the server, not optional
// frontend peer dependencies in the monorepo's installed dependency tree.
const metafile = (await Bun.file('dist/metafile.json').json()) as {
	inputs: Record<string, unknown>;
};
const browserInputs = Bun.file('../server-web/.svelte-kit/client-inputs.json');
if (await browserInputs.exists()) {
	const browser = (await browserInputs.json()) as {
		version: number;
		inputs: string[];
	};
	if (browser.version !== 1)
		throw new Error('Unsupported browser license manifest');
	for (const path of browser.inputs) metafile.inputs[path] = {};
	// Extracted CSS/fonts have no rendered JavaScript module in the browser manifest.
	for (const packageName of [
		'@fontsource-variable/public-sans',
		'tailwindcss',
		'tw-animate-css',
	]) {
		metafile.inputs[
			resolve('../app/node_modules', packageName, 'package.json')
		] = {};
	}
}
const packages = new Map<
	string,
	{ name: string; version: string; license: string; directory: string }
>();
for (const input of Object.keys(metafile.inputs)) {
	if (!input.includes('node_modules/')) continue;
	let directory = dirname(await realpath(resolve(input)));
	while (directory !== dirname(directory)) {
		const manifest = Bun.file(join(directory, 'package.json'));
		if (await manifest.exists()) {
			const pkg = await manifest.json();
			if (pkg.name && pkg.version) {
				packages.set(`${pkg.name}@${pkg.version}`, {
					name: pkg.name,
					version: pkg.version,
					license:
						pkg.name === 'dompurify' &&
						pkg.license === '(MPL-2.0 OR Apache-2.0)'
							? 'Apache-2.0'
							: // 0.10.6 omits package.json license metadata; its shipped LICENSE explicitly grants MIT.
								pkg.name === 'svelte-toolbelt' &&
								  pkg.version === '0.10.6' &&
								  !pkg.license
								? 'MIT'
								: pkg.license,
					directory,
				});
				break;
			}
		}
		directory = dirname(directory);
	}
}
const allowed = new Set([
	'MIT',
	'Apache-2.0',
	'ISC',
	'BSD-2-Clause',
	'BSD-3-Clause',
	'0BSD',
	'MIT-0',
	'Unlicense',
	'(MIT OR Apache-2.0)',
	'OFL-1.1',
]);
let notices = '# Noura server and browser bundled dependency notices\n\n';
for (const [name, pkg] of [...packages].sort(([a], [b]) =>
	a.localeCompare(b),
)) {
	if (!allowed.has(pkg.license))
		throw new Error(`Review the bundled license for ${name}: ${pkg.license}`);
	notices += `## ${name}\n\nLicense: ${pkg.license}\n\n`;
	const files = (await readdir(pkg.directory))
		.filter((name) => /^(licen[cs]e|copying|notice)([.-]|$)/i.test(name))
		.sort();
	if (!files.length) {
		if (name === '@better-auth/utils@0.4.2') {
			notices += await readFile('licenses/better-auth-utils-0.4.2.txt', 'utf8');
		} else if (name === '@tus/utils@0.7.1') {
			notices += await readFile('licenses/tus-utils-0.7.1.txt', 'utf8');
		} else {
			const readme = Bun.file(join(pkg.directory, 'README.md'));
			const text = (await readme.exists()) ? await readme.text() : '';
			const section = text.match(/^## licen[cs]e\s*\n([\s\S]*)/im)?.[1];
			if (section) notices += section + '\n\n';
			else if (!['Unlicense', 'MIT-0', '0BSD'].includes(pkg.license))
				throw new Error(`Missing license text for ${name}`);
		}
	}
	for (const file of files) {
		const path = join(pkg.directory, file);
		if ((await stat(path)).isFile())
			notices += `${await readFile(path, 'utf8')}\n\n`;
	}
}
await writeFile('dist/THIRD_PARTY_NOTICES.md', notices);
console.info(
	`Verified licenses and included notices for ${packages.size} bundled packages`,
);
