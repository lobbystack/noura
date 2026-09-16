import { cp } from 'node:fs/promises';
await cp(
	new URL('../../app/build-hosted', import.meta.url),
	new URL('../dist/public', import.meta.url),
	{ recursive: true },
);
