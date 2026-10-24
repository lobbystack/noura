# Run another plugin's commands

The `command-calls` interface runs a command that another plugin offers, such as creating a task through the Tasks plugin. Import it from `noura:plugin/command-calls@0.1.0`. It needs the `commands` capability, and the capability's `targets` must name the command, as `plugin/command`, or its plugin.

## Functions

- `call(plugin, command, input)`: run `command` of `plugin` with `input`, often JSON, and return its answer. The command runs in that plugin, with that plugin's capabilities, so it can change that plugin's objects for you.

A call can fail with:

- `target-not-declared`: the manifest's targets don't name the command or its plugin.
- `target-unavailable`: the plugin isn't running in this workspace, or doesn't offer the command to other plugins.
- `busy`: the call would come back into a plugin that waits for this one, or the target stayed busy.
- `failed`: the command's own failure, with its message.

## Example

This plugin adds a page with a button that creates a task through the Tasks plugin. Its manifest declares `commands` with the target `tasks/create`, and `views`:

```toml
id = "example-command-calls"
name = "Quick task"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "commands"
targets = ["tasks/create"]

[[capabilities]]
name = "views"
```

The button sends Tasks the task's title. Tasks answers with the task it wrote, as JSON:

```ts
import type { ViewExports } from '@noura/plugin-types';
import { call } from 'noura:plugin/command-calls@0.1.0';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';

let last = 'No task yet.';

export const view: ViewExports = {
	pages: () => [{ id: 'quick-task', title: 'Quick task' }],

	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Quick task' },
			children: new Uint32Array([1, 2]),
		},
		{
			id: 'last',
			kind: { tag: 'text', val: last },
			children: new Uint32Array(),
		},
		{
			id: 'create',
			kind: { tag: 'button', val: 'Add a task to water the plants' },
			children: new Uint32Array(),
		},
	],

	handleEvent(_page: string, event: Event) {
		if (event.tag !== 'pressed' || event.val !== 'create') {
			return;
		}
		try {
			const task = JSON.parse(
				call('tasks', 'create', JSON.stringify({ title: 'Water the plants' })),
			);
			last = `Created ${task.title}.`;
		} catch (error) {
			const payload = (error as { payload?: { tag: string; val?: unknown } })
				.payload;
			last = `Tasks didn't create it: ${payload?.tag ?? String(error)}.`;
		}
	},
};
```

## Notes

- The Tasks plugin's `create`, `update`, `complete` and `reopen` commands take and answer JSON objects. Its crate's `commands.rs` describes them.
- The time Noura spends running the other plugin's command doesn't count toward this plugin's call deadline.
