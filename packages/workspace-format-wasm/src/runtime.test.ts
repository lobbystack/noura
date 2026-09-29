import { existsSync } from 'node:fs';
import { test } from 'bun:test';

const built = existsSync(
	new URL('../wasm/workspace_format_wasm_bg.wasm', import.meta.url),
);

// CI runs `bun run check`, which builds the wasm module, before `bun test`.
// Run `bun run wasm:build` locally to include these fixtures.
test.skipIf(!built)(
	'the wasm build accepts exactly the shared conformance fixtures',
	async () => {
		await import('../scripts/test-wasm-runtime');
	},
);
