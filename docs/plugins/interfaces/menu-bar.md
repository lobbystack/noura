# Add items to the menu bar

The `menu-bar` interface adds items to Noura's menu in the macOS menu bar, the Windows notification area or the Linux tray. Import it from `noura:plugin/menu-bar@0.1.0`. It needs the `menu-bar-items` capability; without it, the call throws `capability-not-declared` with `menu-bar-items`.

Noura stays in the menu bar while every window is closed, so a shortcut and a recording keep working. Choosing an item hands your plugin a [kernel event](kernel-events.md), which counts as something you did.

## Functions

- `setEntries(entries)`: show these entries, in order, in place of the ones your plugin showed before. Each entry has an `id`, unique among your entries, a `kind`, a `label`, whether it's `enabled`, an optional check mark (`checked`) and an optional `parent`, the submenu it goes in. A kind is `item`, which you choose, `submenu`, which opens the entries that name it as their parent, or `separator`, a line with an empty label. Noura shows each plugin's entries together, above Settings… and Quit. An empty list takes yours away.

A list that can't make a menu throws `failed`: an ID twice, an item or a submenu without a label, a parent that isn't an earlier submenu, or more than 200 entries.

## Example

This plugin adds a submenu to pick a color, and shows the one you picked on its page. Its manifest declares `menu-bar-items` and `views`:

```toml
id = "example-menu-bar"
name = "Colors"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "menu-bar-items"

[[capabilities]]
name = "views"
```

```ts
import type {
	KernelEventsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import type { KernelEvent } from 'noura:plugin/kernel-events@0.1.0';
import { setEntries } from 'noura:plugin/menu-bar@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

const COLORS = ['Red', 'Green', 'Blue'];
let picked = 'Red';

/** Show the submenu, with a check mark on the color you picked. */
function showMenu(): void {
	try {
		setEntries([
			{ id: 'colors', kind: { tag: 'submenu' }, label: 'Color', enabled: true },
			...COLORS.map((color) => ({
				id: color,
				kind: { tag: 'item' as const },
				label: color,
				enabled: true,
				checked: color === picked,
				parent: 'colors',
			})),
		]);
	} catch {
		// Noura shows no menu bar here.
	}
}

export const kernelEvents: KernelEventsExports = {
	handleKernelEvent(event: KernelEvent) {
		if (event.tag === 'menu-item-chosen' && COLORS.includes(event.val)) {
			picked = event.val;
			showMenu();
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'colors', title: 'Colors' }],
	render(): Node[] {
		showMenu();
		return [
			{
				id: 'root',
				kind: { tag: 'panel', val: 'Colors' },
				children: new Uint32Array([1]),
			},
			{
				id: 'picked',
				kind: { tag: 'text', val: `You picked ${picked}.` },
				children: new Uint32Array(),
			},
		];
	},
};
```

## Notes

- The mock host keeps your entries in `host.menuEntries`. Choose one with `host.kernelEvent({ tag: 'menu-item-chosen', val: 'Green' })`.
