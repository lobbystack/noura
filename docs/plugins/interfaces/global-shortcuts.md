# Answer keyboard shortcuts anywhere

The `global-shortcuts` interface lists the keyboard shortcuts your plugin answers in any app, even with Noura's windows closed. Import it from `noura:plugin/global-shortcuts@0.1.0`. It needs the `global-shortcuts` capability, whose `shortcuts` list declares each shortcut with an `id`, a `title` and the `keys` it starts with, as the example's manifest shows.

Keys are modifiers among `Ctrl`, `Alt`, `Shift` and `Cmd`, joined by `+`, then one key: a capital letter, a digit, `F1` to `F24`, or `Space`, `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, an arrow such as `Up`, `Home`, `End`, `PageUp` or `PageDown`. You can change them in Settings > Speech. An update that adds a shortcut waits for your approval.

Noura hands each press and release to your plugin as a [kernel event](kernel-events.md), which counts as something you did. It goes to the workspace you used last that has your plugin on. With no such workspace, the shortcut does nothing.

Under Wayland, where apps can't see keys pressed elsewhere, you bind `noura --shortcut <plugin>/<shortcut>` in the desktop's settings, and each run sends a press, then a release.

## Functions

- `shortcuts()`: the shortcuts the manifest declares, each with its `id`, `title`, the `keys` it answers to now, and whether Noura could register them with the system (`registered`). Another app may hold the keys already.

## Example

This plugin adds a page that lists its shortcut and counts its presses. Its manifest declares `global-shortcuts` and `views`:

```toml
id = "example-shortcuts"
name = "Shortcuts"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "global-shortcuts"
shortcuts = [{ id = "count", title = "Count", keys = "Ctrl+Alt+K" }]

[[capabilities]]
name = "views"
```

```ts
import type {
	KernelEventsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import { shortcuts } from 'noura:plugin/global-shortcuts@0.1.0';
import type { KernelEvent } from 'noura:plugin/kernel-events@0.1.0';
import type { Node, NodeKind } from 'noura:plugin/ui@0.1.0';

let presses = 0;

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

/** Each shortcut and its keys, or why Noura can't list them. */
function listed(): string {
	try {
		return shortcuts()
			.map((shortcut) => `${shortcut.title}: ${shortcut.keys}`)
			.join(', ');
	} catch {
		return 'Noura can’t list the shortcuts here.';
	}
}

export const kernelEvents: KernelEventsExports = {
	handleKernelEvent(event: KernelEvent) {
		if (event.tag === 'shortcut-pressed' && event.val === 'count') {
			presses += 1;
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'shortcuts', title: 'Shortcuts' }],
	render: () => [
		node('root', { tag: 'panel', val: 'Shortcuts' }, [1, 2]),
		node('keys', { tag: 'text', val: listed() }),
		node('count', { tag: 'text', val: `Pressed ${presses} times.` }),
	],
};
```

## Notes

- The mock host lists the manifest's shortcuts as registered. Send a press with `host.kernelEvent({ tag: 'shortcut-pressed', val: 'count' })`.
