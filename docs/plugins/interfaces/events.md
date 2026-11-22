# Tell Noura about events

The `events` interface tells Noura that something happened in your plugin, so the Orbs whose triggers name it can act. Import it from `noura:plugin/events@0.1.0`. It needs the `events` capability, whose manifest entry declares each event with an `id` and a `title`; without it, the call throws `capability-not-declared` with `events`.

An Orb's trigger names your event as `<plugin>.<event>`, such as `transcription.finished` for the Transcription plugin's `finished`. The trigger list shows the event's `title`. Your plugin never starts an Orb itself: Noura starts the Orbs whose triggers name the event, while it's open on this device and nothing holds them.

## Functions

- `emit(event, object)`: tell Noura that `event`, one your manifest declares, happened, about the object with the stable ID `object`, or about nothing with `undefined`. The call returns at once, and Noura starts the Orbs after it. An Orb that may read the object gets its title, a link to it and its body in the run's first message. An event your manifest doesn't declare throws `failed`.

Noura starts each Orb once for an object as it stands: an event about the same object starts the Orb again only after the object's file changes. An event that happens during a tool call of an Orb's run starts no Orb, so two Orbs can't start each other in a loop.

## Example

This plugin adds a command to the palette that tells your Orbs a standup ended. An Orb whose trigger says `event example-events.standup-done` starts. Its manifest declares `events` and `commands`:

```toml
id = "example-events"
name = "Standups"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "events"
events = [{ id = "standup-done", title = "Standup ends" }]

[[capabilities]]
name = "commands"
```

```ts
import type { CommandsExports } from '@noura/plugin-types/0.1.0';
import { emit } from 'noura:plugin/events@0.1.0';

export const commands: CommandsExports = {
	commands: () => [
		{ id: 'standup-done', title: 'Standup done', palette: true, plugins: false },
	],

	run(id: string, _input: string): string {
		if (id !== 'standup-done') {
			throw `Standups has no command ${id}.`;
		}
		emit('standup-done', undefined);
		return 'Your Orbs heard.';
	},
};
```

## Notes

- The mock host keeps what your plugin emits in `host.emitted`, oldest first, and starts no Orb.
- You approve `events` like any capability. An update that declares a new event waits for your approval.
