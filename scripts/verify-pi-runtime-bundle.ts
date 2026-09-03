import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const bundlePath = resolve(
	import.meta.dirname,
	'../apps/app/dist/pi-runtime-spike/noura-pi-runtime-spike.js',
);
const source = await readFile(bundlePath, 'utf8');
const forbiddenRuntimeImports = [
	'node:child_process',
	'node:fs',
	'node:net',
	'node:tls',
	'child_process',
	'process.env',
];
const found = forbiddenRuntimeImports.filter((value) => source.includes(value));

if (found.length > 0) {
	throw new Error(
		`Pi runtime bundle contains Node-only runtime references: ${found.join(', ')}`,
	);
}

console.log(
	`Pi runtime bundle is browser-safe: ${(source.length / 1024).toFixed(2)} KiB uncompressed.`,
);
