import { defineConfig } from 'vitest/config';

export default defineConfig({
	test: {
		include: ['tests/**/*.test.ts'],
		// Each test builds plugins and starts the real host, which compiles
		// them.
		testTimeout: 180_000,
		hookTimeout: 180_000,
	},
});
