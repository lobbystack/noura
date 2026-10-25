# Offer tools to AI assistants

The `tools` interface is what a plugin offers the AI assistants that reach Noura through MCP. Export it from `src/plugin.ts` as `tools`, typed `ToolsExports`. Noura calls it only when the manifest declares the `tools` capability.

## Functions

- `tools()`: the tools the plugin offers. Each has a name of lowercase letters, digits, hyphens and underscores, unique within the plugin, a description for the assistant, and a JSON Schema, as JSON text, that the arguments must match.
- `callTool(name, args)`: run a tool with arguments, as JSON text. Noura checks them against the tool's schema before the plugin sees them. Return the answer, or throw a string that says why the tool failed. Both go back to the assistant.

## Example

This plugin offers a tool that counts the files in a folder of the workspace. Its manifest declares `tools`, and `workspace-read` to list the files:

```toml
id = "example-tools"
name = "File count"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "tools"

[[capabilities]]
name = "workspace-read"
```

The schema makes `folder` optional, so the tool counts the whole workspace when the assistant leaves it out:

```ts
import type { ToolsExports } from '@noura/plugin-types/0.1.0';
import { listEntries } from 'noura:plugin/workspace@0.1.0';

export const tools: ToolsExports = {
	tools: () => [
		{
			name: 'count_files',
			description:
				'Counts the files in a folder of the workspace, or in the whole workspace.',
			inputSchema: JSON.stringify({
				type: 'object',
				properties: {
					folder: {
						type: 'string',
						description: 'A workspace-relative folder, such as Notes.',
					},
				},
			}),
		},
	],

	callTool(name: string, args: string): string {
		if (name !== 'count_files') {
			throw `There's no tool ${name}.`;
		}
		const { folder } = JSON.parse(args) as { folder?: string };
		const files = listEntries().filter(
			(entry) =>
				entry.tag === 'ok' &&
				entry.val.kind === 'file' &&
				(!folder || entry.val.path.startsWith(`${folder}/`)),
		);
		return JSON.stringify({ files: files.length });
	},
};
```

## Notes

- A plugin's tools appear to assistants while the plugin is on in the open workspace.
- A tool that changes files writes objects of the plugin's own types through [`objects`](objects.md), as any other call does.
