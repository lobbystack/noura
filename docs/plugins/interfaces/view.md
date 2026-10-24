# Add pages to the rail

The `view` interface is what a plugin offers Noura to add pages to the rail. Export it from `src/plugin.ts` as `view`, typed `ViewExports`. Noura calls it only when the manifest declares the `views` capability.

## Functions

- `pages()`: the pages the plugin adds, each with an ID unique within the plugin and the title the rail and the page's tab show.
- `render(page)`: the [nodes](ui.md) of a page's view.
- `sidebar(page)`: the nodes of the section the page adds to the sidebar, above the file tree, while the page shows. Return none for no section. The section can't hold a virtual list, and its node IDs must differ from the page's.
- `rows(page, node, start, count)`: rows `start` up to `start + count` of the virtual list `node`, as they scroll into view. Return fewer when the list ends sooner.
- `handleEvent(page, event)`: something you did in the page or its sidebar section. Noura draws the plugin's views again afterwards.

Every function is optional. The build fills in one that returns nothing.

## Example

This plugin adds a page with a virtual list of 10,000 numbers and a sidebar section that names the number you selected. Its manifest declares `views`:

```toml
id = "example-view"
name = "Numbers"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "views"
```

`render` returns only the list's size. Noura asks `rows` for the rows it shows:

```ts
import type { ViewExports } from '@noura/plugin-types';
import type { Event, Node, VirtualRow } from 'noura:plugin/ui@0.1.0';

const COUNT = 10_000;
let selected: string | undefined;

export const view: ViewExports = {
	pages: () => [{ id: 'numbers', title: 'Numbers' }],

	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Numbers' },
			children: new Uint32Array([1]),
		},
		{
			id: 'numbers',
			kind: { tag: 'virtual-list', val: { label: 'Numbers', rows: COUNT } },
			children: new Uint32Array(),
		},
	],

	rows(
		_page: string,
		_node: string,
		start: number,
		count: number,
	): VirtualRow[] {
		const end = Math.min(start + count, COUNT);
		const rows: VirtualRow[] = [];
		for (let number = start; number < end; number += 1) {
			rows.push({
				id: String(number + 1),
				item: { label: `Number ${number + 1}` },
			});
		}
		return rows;
	},

	sidebar: (): Node[] => [
		{
			id: 'side',
			kind: { tag: 'panel', val: 'Selection' },
			children: new Uint32Array([1]),
		},
		{
			id: 'chosen',
			kind: { tag: 'text', val: selected ?? 'Select a number.' },
			children: new Uint32Array(),
		},
	],

	handleEvent(_page: string, event: Event) {
		if (event.tag === 'selected') {
			selected = `You selected number ${event.val.item}.`;
		}
	},
};
```

## Notes

- Noura draws a plugin's views again after each event and each change to the workspace's files. Until the plugin's new view arrives, the page keeps showing the last one.
- A plugin can open its own pages and objects in answer to an event with the [`navigation`](navigation.md) interface.
