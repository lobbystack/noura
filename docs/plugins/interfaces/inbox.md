# Add sections to the Inbox

The `inbox` interface is what a plugin offers Noura to add sections to the Inbox, the page you start each day on. Export it from `src/plugin.ts` as `inbox`, typed `InboxExports`. Noura calls it only when the manifest declares the `inbox-sections` capability.

## Functions

- `sections()`: the sections the plugin adds, each with an ID unique within the plugin and the heading the Inbox shows above it.
- `renderSection(section)`: the [nodes](ui.md) of a section. Noura draws the sections again when the workspace's files change.
- `handleSectionEvent(section, event)`: something you did in a section. Noura draws the plugin's views again afterwards.

## Example

This plugin adds an Inbox section that lists the notes waiting in the workspace's `Inbox` folder. Its manifest declares `inbox-sections`, and `workspace-read` to list the folder:

```toml
id = "example-inbox"
name = "Inbox folder"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "inbox-sections"

[[capabilities]]
name = "workspace-read"
```

`renderSection` shows a line of text when the folder is empty, and a list otherwise:

```ts
import type { InboxExports } from '@noura/plugin-types';
import type { Node } from 'noura:plugin/ui@0.1.0';
import { listEntries } from 'noura:plugin/workspace@0.1.0';

export const inbox: InboxExports = {
	sections: () => [{ id: 'waiting', title: 'Waiting in Inbox/' }],

	renderSection(): Node[] {
		const waiting = listEntries()
			.flatMap((entry) => (entry.tag === 'ok' && entry.val.kind === 'file' ? [entry.val.path] : []))
			.filter((path) => path.startsWith('Inbox/'))
			.sort();
		if (waiting.length === 0) {
			return [{ id: 'empty', kind: { tag: 'text', val: 'Nothing is waiting in Inbox/.' }, children: new Uint32Array() }];
		}
		return [
			{
				id: 'waiting',
				kind: { tag: 'list', val: { label: 'Waiting in Inbox/', reorder: false, opensOnClick: false } },
				children: new Uint32Array(waiting.map((_, index) => index + 1)),
			},
			...waiting.map((path): Node => ({
				id: path,
				kind: { tag: 'item', val: { label: path.slice('Inbox/'.length) } },
				children: new Uint32Array(),
			})),
		];
	},
};
```

## Notes

- A section that answers `activated` on an item can open it with [`navigation`](navigation.md); the manifest then needs `views` too.
- The mock host draws a section with `host.inboxSection(id)` and sends it events with `host.sendInbox(id, event)`.
