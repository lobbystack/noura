import { fileURLToPath } from 'node:url';

if (!process.env.NOURA_TEST_DATABASE_URL) {
	throw new Error(
		'NOURA_TEST_DATABASE_URL must name a dedicated PostgreSQL test database',
	);
}
for (const variable of [
	'NOURA_NATIVE_PROBE',
	'NOURA_NATIVE_SIGNIN_PROBE',
	'NOURA_NATIVE_KEYS_PROBE',
]) {
	const path = process.env[variable];
	if (!path || !(await Bun.file(path).exists())) {
		throw new Error(
			`${variable} must point to its compiled native example; run cargo build -p local-core --examples first`,
		);
	}
}
const result = Bun.spawnSync(
	[
		'bun',
		'test',
		'--max-concurrency=1',
		'src/protocol.test.ts',
		'src/store.integration.test.ts',
		'src/auth.integration.test.ts',
		'src/native.integration.test.ts',
		'src/access.integration.test.ts',
		'src/checkpoints.test.ts',
		'src/checkpoints.integration.test.ts',
		'src/collaboration-limits.integration.test.ts',
		'src/invitations.integration.test.ts',
		'src/signin.integration.test.ts',
		'src/public-links.integration.test.ts',
		'src/keys.integration.test.ts',
		'src/blobs.integration.test.ts',
		'src/blob-read.test.ts',
	],
	{
		cwd: fileURLToPath(new URL('..', import.meta.url)),
		stdout: 'inherit',
		stderr: 'inherit',
	},
);
process.exit(result.exitCode);
