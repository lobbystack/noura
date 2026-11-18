# Show notifications with actions

The `notifications` interface shows notifications in the system's notification center: Notification Center on macOS, the Windows notification center, or the desktop's notification server on Linux. Import it from `noura:plugin/notifications@0.1.0`. It needs the `notifications` capability; without it, the call throws `capability-not-declared` with `notifications`.

Noura shows a notification only in answer to something you did: a [kernel event](kernel-events.md), such as a shortcut press, or an event in one of your views or panels. Any other call fails with `failed`. When you choose the notification or one of its actions, your plugin gets a `notification-chosen` kernel event, which counts as something you did, so your plugin may open things or record in answer. When you turned off Noura's notifications in the system's settings, nothing shows and the call still succeeds.

## Functions

- `notify(notification)`: show a notification with an `id`, unique among yours, a `title`, a `body` of one sentence or empty, and up to 3 `actions`, each with an `id` and a `label` of one or two words. A notification with the ID of one that still shows takes its place. Some systems show only the first action.
- `withdraw(id)`: take away the notification with this ID, if it still shows.

A notification that breaks these rules throws `failed`: an empty ID or title, more than 3 actions, an action without an ID or a label, the same action ID twice, or text over 300 characters.

## Example

This plugin shows a notification when you press its shortcut, and counts the times you chose Wave. Its manifest declares `global-shortcuts`, `notifications` and `views`:

```toml
id = "example-notifications"
name = "Waves"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "global-shortcuts"
shortcuts = [{ id = "hello", title = "Hello", keys = "Ctrl+Alt+H" }]

[[capabilities]]
name = "notifications"

[[capabilities]]
name = "views"
```

The shortcut's press shows the notification, and a `notification-chosen` event with the action `wave` counts one more wave:

```ts
import type {
	KernelEventsExports,
	ViewExports,
} from '@noura/plugin-types/0.1.0';
import type { KernelEvent } from 'noura:plugin/kernel-events@0.1.0';
import { notify } from 'noura:plugin/notifications@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

let waves = 0;

export const kernelEvents: KernelEventsExports = {
	handleKernelEvent(event: KernelEvent) {
		if (event.tag === 'shortcut-pressed' && event.val.id === 'hello') {
			notify({
				id: 'hello',
				title: 'Hello',
				body: '',
				actions: [{ id: 'wave', label: 'Wave' }],
			});
		} else if (
			event.tag === 'notification-chosen' &&
			event.val.action === 'wave'
		) {
			waves += 1;
		}
	},
};

export const view: ViewExports = {
	pages: () => [{ id: 'waves', title: 'Waves' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Waves' },
			children: new Uint32Array([1]),
		},
		{
			id: 'count',
			kind: { tag: 'text', val: `Waved ${waves} times.` },
			children: new Uint32Array(),
		},
	],
};
```

## Behavior on each system

- On macOS, Noura asks for permission to show notifications the first time. A development build that runs outside an app bundle shows none.
- On Linux, choosing the notification itself works on every notification server, but some servers show no action buttons.
- The mock host keeps the notifications that show in `host.notifications`, by ID. Choose one with `host.chooseNotification(id, action)`.
