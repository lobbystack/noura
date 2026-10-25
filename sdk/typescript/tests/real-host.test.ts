// The tests' real host fails a request when noura-kit-host stops, with
// what the host wrote, instead of leaving the test to time out.
import { join } from 'node:path';
import { expect, test } from 'vitest';

import { KIT } from './support/kit.js';
import { RealHost } from './support/real-host.js';

const STOPPING_HOST = join(KIT, 'tests/support/stopping-host.mjs');

test(
	'a host that stops fails the requests waiting on it and later ones',
	{ timeout: 10_000 },
	async () => {
		const host = await RealHost.start({}, STOPPING_HOST);
		if (!host) {
			throw new Error('The stand-in host didn’t start.');
		}
		try {
			await expect(host.pages()).rejects.toThrow(
				'The kit host stopped with 3: The stand-in host stopped on purpose.',
			);
			await expect(host.plugin('word-count')).rejects.toThrow(
				'The kit host stopped with 3',
			);
		} finally {
			await host.close();
		}
	},
);
