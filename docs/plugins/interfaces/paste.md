# Paste into any app

The `paste` interface pastes text into the app you're typing in, anywhere on the computer, and puts text on the clipboard. Import it from `noura:plugin/paste@0.1.0`. Each call needs the `paste` capability; without it, the call throws `capability-not-declared` with `paste`.

Noura pastes only in answer to something you did: a [kernel event](kernel-events.md), such as a shortcut press, or an event in one of your plugin's views or panels. Any other paste throws `failed`.

## Functions

- `paste(text, method)`: paste `text` into the focused app. With `clipboard`, Noura puts the text on the clipboard, presses the paste keys and puts back what the clipboard held: on macOS once the app has read it, on Windows and Linux after half a second. On Linux it presses Shift+Insert and puts back the primary selection too. With `typing`, Noura types the text key by key and leaves the clipboard alone, for apps that refuse a paste. The call returns once the text is in, and the wait doesn't count toward the call's deadline.
- `copy(text)`: put `text` on the clipboard and leave it there, as Copy does.

## Example

This plugin pastes a greeting into the app you used last when you press its shortcut. Its manifest declares `global-shortcuts`, `paste` and `views`:

```toml
id = "example-paste"
name = "Greeter"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "global-shortcuts"
shortcuts = [{ id = "greet", title = "Greet", keys = "Ctrl+Alt+G" }]

[[capabilities]]
name = "paste"

[[capabilities]]
name = "views"
```

```ts
import type {
	KernelEventsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import type { KernelEvent } from 'noura:plugin/kernel-events@0.1.0';
import { paste } from 'noura:plugin/paste@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

let last = 'Press Ctrl+Alt+G in any app.';

export const kernelEvents: KernelEventsExports = {
	handleKernelEvent(event: KernelEvent) {
		if (event.tag !== 'shortcut-pressed' || event.val !== 'greet') {
			return;
		}
		try {
			paste('Hello from Noura!', 'clipboard');
			last = 'Pasted.';
		} catch {
			last = 'Noura couldn’t paste.';
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'greeter', title: 'Greeter' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Greeter' },
			children: new Uint32Array([1]),
		},
		{
			id: 'last',
			kind: { tag: 'text', val: last },
			children: new Uint32Array(),
		},
	],
};
```

## Notes

- On macOS, Noura needs the Accessibility permission to press keys in another app. It asks the first time.
- The mock host records what you paste in `host.pastes` and what you copy in `host.copies`.
