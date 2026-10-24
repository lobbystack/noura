# Add a page to Settings

The `settings` interface is what a plugin offers Noura to add its own page to Settings. Export it from `src/plugin.ts` as `settings`, typed `SettingsExports`. Noura calls it only when the manifest declares the `settings` capability.

Noura keeps no setting values for a plugin. A plugin keeps them in workspace files, as an object of its own type, so they sync and back up with the workspace.

## Functions

- `page()`: the title of the plugin's page in Settings, or `undefined` for no page.
- `renderSettings()`: the [nodes](ui.md) of the page.
- `handleSettingsEvent(event)`: something you did on the page. Noura draws the plugin's views again afterwards.

## Example

This plugin adds a Settings page with the greeting its rail page shows. Its manifest declares `settings` and `views`:

```toml
id = "example-settings"
name = "Greeting"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "settings"

[[capabilities]]
name = "views"
```

The example keeps the greeting in memory, so it resets when Noura restarts. To keep it, store it in an object of the plugin's own type, as the [`objects`](objects.md) page shows:

```ts
import type { SettingsExports, ViewExports } from '@noura/plugin-types';
import type { Event, Node } from 'noura:plugin/ui@0.1.0';

let greeting = 'Good morning';

export const settings: SettingsExports = {
	page: () => 'Greeting',

	renderSettings: (): Node[] => [
		{ id: 'root', kind: { tag: 'panel', val: 'Greeting' }, children: new Uint32Array([1]) },
		{
			id: 'greeting',
			kind: { tag: 'input', val: { label: 'Greeting', value: { tag: 'text', val: greeting }, choices: [] } },
			children: new Uint32Array(),
		},
	],

	handleSettingsEvent(event: Event) {
		if (event.tag === 'changed' && event.val.node === 'greeting' && event.val.value.tag === 'text') {
			greeting = event.val.value.val;
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'greeting', title: 'Greeting' }],
	render: (): Node[] => [
		{ id: 'root', kind: { tag: 'panel', val: 'Greeting' }, children: new Uint32Array([1]) },
		{ id: 'text', kind: { tag: 'text', val: `${greeting}!` }, children: new Uint32Array() },
	],
};
```

## Notes

- Noura draws the plugin's rail pages again after each Settings event, so the page shows the new greeting.
- The mock host draws the page with `host.settings()` and sends it events with `host.sendSettings(event)`.
