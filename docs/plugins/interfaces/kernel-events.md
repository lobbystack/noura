# Answer events from outside your views

The `kernel-events` interface is one your plugin exports: Noura calls `handleKernelEvent` for things that happen outside your views, such as a shortcut press, a menu bar item or notification you chose, or another app turning on the microphone. Export it as `kernelEvents`, typed with `KernelEventsExports` from `@noura/plugin-types/0.1.0`.

Noura calls it only for the capabilities your manifest declares, and each event counts as something you did, so your plugin may paste or open things in answer. Noura draws your views again afterwards.

## Events

- `shortcut-pressed` and `shortcut-released`, with the shortcut's `id` and the time its keys went down or came up (`atMs`), in milliseconds on a clock that starts with Noura. Compare a press's time with its release's to tell a tap from a hold. Need `global-shortcuts`.
- `menu-item-chosen`, with the menu bar entry's ID. Needs `menu-bar-items`.
- `notification-chosen`, with the notification's `id` and the `action` you chose, or none when you chose the notification itself. Needs `notifications`.
- `microphone-in-use`, with the name of the app when the system gives it (`app`), when another app turns on the microphone, as for a call. Noura never records by itself, so your plugin may ask you first. Needs `speech`.
- `microphone-free`, when no other app uses the microphone. Needs `speech`.
- `recording-changed`, with a recording's ID, when it changes state, as when it finishes after `stop`. Needs `speech`.

## Example

This plugin counts the times another app turned on the microphone. Its manifest declares `speech` and `views`:

```toml
id = "example-kernel-events"
name = "Microphone"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "speech"

[[capabilities]]
name = "views"
```

```ts
import type {
	KernelEventsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import type { KernelEvent } from 'noura:plugin/kernel-events@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

let calls = 0;
let lastApp = 'No app yet';

export const kernelEvents: KernelEventsExports = {
	handleKernelEvent(event: KernelEvent) {
		if (event.tag === 'microphone-in-use') {
			calls += 1;
			lastApp = event.val.app ?? 'An app';
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'microphone', title: 'Microphone' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Microphone' },
			children: new Uint32Array([1]),
		},
		{
			id: 'count',
			kind: {
				tag: 'text',
				val: `Turned on ${calls} times. Last: ${lastApp}.`,
			},
			children: new Uint32Array(),
		},
	],
};
```

## Notes

- A plugin built before this interface existed exports none, and gets no kernel events.
- In the mock host, send an event with `host.kernelEvent(event)`. An event whose capability your manifest doesn't declare throws a `CapabilityError`.
