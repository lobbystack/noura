# Offer commands

The `commands` interface is what a plugin offers the command palette and other plugins. Export it from `src/plugin.ts` as `commands`, typed `CommandsExports`. Noura calls it only when the manifest declares the `commands` capability.

## Functions

- `commands()`: the commands the plugin offers. Each has an ID of lowercase letters, digits and hyphens, unique within the plugin, the title the palette shows, whether the palette lists it (`palette`) and whether other plugins may call it (`plugins`).
- `run(id, input)`: run a command. From the palette `input` is empty; from another plugin it's the text that plugin sent, often JSON. Return the answer, or throw a string that says why the command failed. Noura draws the plugin's views again afterwards.

## Example

This plugin counts: a palette command adds one, and other plugins can read the count. A page shows it. Its manifest declares `commands` and `views`:

```toml
id = "example-commands"
name = "Counter"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "commands"

[[capabilities]]
name = "views"
```

`run` handles both commands and throws for an ID it doesn't know:

```ts
import type { CommandsExports, ViewExports } from '@noura/plugin-types/0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

let count = 0;

export const commands: CommandsExports = {
	commands: () => [
		{ id: 'add-one', title: 'Counter: add one', palette: true, plugins: false },
		{
			id: 'count',
			title: 'Counter: read the count',
			palette: false,
			plugins: true,
		},
	],

	run(id: string, _input: string): string {
		switch (id) {
			case 'add-one':
				count += 1;
				return String(count);
			case 'count':
				return JSON.stringify({ count });
			default:
				throw `The counter has no command ${id}.`;
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'counter', title: 'Counter' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Counter' },
			children: new Uint32Array([1]),
		},
		{
			id: 'count',
			kind: { tag: 'text', val: `The count is ${count}.` },
			children: new Uint32Array(),
		},
	],
};
```

## Notes

- Another plugin calls `count` as `example-commands/count` through the [`command-calls`](command-calls.md) interface, once its manifest names that target. The command runs in this plugin, with this plugin's capabilities.
- A command that runs from the palette counts as something you did, so it may open pages and objects with the [`navigation`](navigation.md) interface.
