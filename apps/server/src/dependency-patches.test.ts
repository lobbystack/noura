import { expect, test } from 'bun:test';

const patchUrl = new URL(
	'../../../patches/better-auth@1.7.2.patch',
	import.meta.url,
);
const manifestUrl = new URL('../../../package.json', import.meta.url);

test('the better-auth int8 patch is present and wired into patchedDependencies', async () => {
	const patch = await Bun.file(patchUrl).text();
	// The Postgres number allowlist must accept the internal Postgres type names,
	// otherwise a bigint column is reported as int8 and warns as a mismatch.
	expect(patch).toContain('"int8"');
	expect(patch).toContain('"int2"');
	expect(patch).toContain('"float8"');

	const manifest = (await Bun.file(manifestUrl).json()) as {
		patchedDependencies?: Record<string, string>;
	};
	expect(manifest.patchedDependencies?.['better-auth@1.7.2']).toBe(
		'patches/better-auth@1.7.2.patch',
	);
});
