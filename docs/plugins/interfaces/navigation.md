# Open pages and objects

The `navigation` interface shows things in the window you acted in. Import it from `noura:plugin/navigation@0.1.0`. It needs the `views` capability, and `trashObject` needs `object-types` too.

Noura acts only for a call that answers something you did: an event in one of the plugin's views or panels, or a command you ran from the palette. Any other call fails with `failed`, so a plugin can't open things on its own. Noura acts once the plugin's call returns.

## Functions

- `openObject(id, placement)`: open the object with this stable ID in the note editor, with its properties panel beside its body. `preview` opens it in the preview tab, as a click in the file tree does; `new-tab` opens a tab that stays.
- `openPage(page)`: show the plugin's own page with this ID.
- `trashObject(id)`: move an object of the plugin's own types to the trash, as Move to Trash in the file tree does. Noura first saves what you typed in it and closes its tabs.

## Example

This plugin adds two pages, each with a button that opens the other. Its manifest declares `views`:

```toml
id = "example-navigation"
name = "Two pages"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "views"
```

`handleEvent` answers the press, so Noura opens the page once the call returns:

```ts
import type { ViewExports } from '@noura/plugin-types';
import { openPage } from 'noura:plugin/navigation@0.1.0';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';

const OTHER: Record<string, string> = { first: 'second', second: 'first' };

export const view: ViewExports = {
	pages: () => [
		{ id: 'first', title: 'First page' },
		{ id: 'second', title: 'Second page' },
	],

	render: (page: string): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: `The ${page} page` },
			children: new Uint32Array([1]),
		},
		{
			id: 'open',
			kind: { tag: 'button', val: `Open the ${OTHER[page]} page` },
			children: new Uint32Array(),
		},
	],

	handleEvent(page: string, event: Event) {
		const other = OTHER[page];
		if (event.tag === 'pressed' && event.val === 'open' && other) {
			openPage(other);
		}
	},
};
```

## Notes

- To open an object you create, pass the `id` that the [`objects`](objects.md) interface returns.
- In the mock host, `host.openings` lists what the plugin asked to open or trash, in order.
