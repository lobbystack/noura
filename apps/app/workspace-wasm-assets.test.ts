import { expect, test } from 'bun:test';
import { workspaceWasmAssets } from './workspace-wasm-assets';

test('production assets include the glue, binary, and every relative JS snippet', () => {
	const plugin = workspaceWasmAssets();
	const emitted = new Map<string, string>();
	const hook = plugin.generateBundle;
	if (typeof hook !== 'function') throw new Error('Missing asset emitter');
	Reflect.apply(
		hook,
		{
			emitFile(asset: { fileName: string; source: Uint8Array }) {
				emitted.set(asset.fileName, new TextDecoder().decode(asset.source));
			},
		},
		[],
	);
	const glue = [...emitted.keys()].find((path) =>
		path.endsWith('/workspace_format_wasm.js'),
	)!;
	expect(glue).toMatch(/^workspace-wasm\/[a-f0-9]{16}\//);
	const directory = glue.slice(0, glue.lastIndexOf('/') + 1);
	expect(emitted.has(directory + 'workspace_format_wasm_bg.wasm')).toBe(true);
	const imports = [
		...emitted.get(glue)!.matchAll(/from ['"]\.\/([^'"]+)['"]/g),
	];
	expect(imports.length).toBeGreaterThan(0);
	for (const match of imports)
		expect(emitted.has(directory + match[1])).toBe(true);
});
