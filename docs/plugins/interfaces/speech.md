# Record and transcribe speech

The `speech` interface records the microphone and the sound other apps play, and turns speech into text with models on this device. Import it from `noura:plugin/speech@0.1.0`. Each call needs the `speech` capability; without it, the call throws `capability-not-declared` with `speech`.

Noura records and transcribes on threads of its own, so each call returns at once. Start a recording, then read its text whenever Noura draws your views again or hands you a kernel event. Nothing you say leaves the device.

Models live in the app data folder on this device, never in the workspace, and never sync. You download and delete them in Settings > Speech, which also holds the language you speak.

## Functions

- `models()`: the models in Noura's catalog, the downloaded ones first. Each has an `id`, such as `parakeet-ultra`, a `name`, its `languages`, its `size` in bytes, whether it shows text while you speak (`streaming`), whether it translates into English (`translation`), whether it's `downloaded` and whether Noura `recommended` it for your language. The catalog holds only models whose license allows commercial use.
- `microphones()`: the microphones this computer has now, each with an `id`, a `name` and whether it's the default (`isDefault`).
- `readPreferences()` and `setPreferences(preferences)`: your plugin's choices on this device: a `model`, a `microphone`, whether recordings hear the system audio (`systemAudio`) and a `language`. Leave one out for Noura's recommendation or the computer's default. Noura keeps them in the app data folder.
- `start(options)`: start a recording and get its ID. Pass the `sources`, `microphone` and `system-audio`, and optionally a `model`, a `microphone` and a `language` in place of your preferences. It throws `failed` when the model isn't downloaded or a microphone can't open.
- `readRecording(id)`: the recording as it stands: its `state`, how long it recorded (`elapsedMs`), the `segments` Noura is sure of and the `pending` text a streaming model may still change. Each segment has a `channel`, `me` for the microphone and `others` for the system audio, a start and an end in milliseconds, and its `text`.
- `stop(id)`: stop recording. Noura transcribes what's left, and the recording turns `finished`.
- `cancel(id)`: stop recording and drop its audio and text.
- `saveAudio(id, object)`: write a finished recording's audio as a WAV file beside one of your plugin's objects, and get its path. Needs `object-types` too.
- `keepAudio(id, days)`: keep a finished recording's audio on this device for that many days.
- `forget(id)`: forget a recording that ended, with its audio unless you kept it.
- `rememberDictation(text)` and `lastDictation()`: keep one piece of text on this device, so it survives a crash, and read it back.

A recording another plugin started throws `not-found`.

## Example

This plugin adds a page that records you and shows the text so far. Its manifest declares `speech` and `views`:

```toml
id = "example-speech"
name = "Listen"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "speech"

[[capabilities]]
name = "views"
```

`handleEvent` starts and stops the recording, and `render` reads it each time Noura draws the page:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import {
	readRecording,
	start,
	stop,
} from 'noura:plugin/speech@0.1.0';
import type { Event, Node, NodeKind } from 'noura:plugin/ui@0.1.0';

/** The recording you started last. */
let recording: string | undefined;
/** Why the last start failed. */
let problem: string | undefined;

function node(id: string, kind: NodeKind, children: number[] = []): Node {
	return { id, kind, children: new Uint32Array(children) };
}

/** The text so far, or why there is none. */
function heard(): string {
	if (problem) {
		return problem;
	}
	if (recording === undefined) {
		return 'Press Start and speak.';
	}
	const now = readRecording(recording);
	const text = [...now.segments, ...now.pending]
		.map((segment) => segment.text)
		.join(' ');
	return text || 'Listening…';
}

export const view: ViewExports = {
	pages: () => [{ id: 'listen', title: 'Listen' }],

	render(): Node[] {
		const label = recording === undefined ? 'Start' : 'Stop';
		return [
			node('root', { tag: 'panel', val: 'Listen' }, [1, 2]),
			node('toggle', { tag: 'button', val: label }),
			node('heard', { tag: 'text', val: heard() }),
		];
	},

	handleEvent(_page: string, event: Event) {
		if (event.tag !== 'pressed' || event.val !== 'toggle') {
			return;
		}
		try {
			if (recording === undefined) {
				recording = start({ sources: ['microphone'] });
				problem = undefined;
			} else {
				stop(recording);
				recording = undefined;
			}
		} catch (error) {
			const payload = (error as { payload?: { val?: string } }).payload;
			problem = payload?.val ?? 'Noura couldn’t record.';
		}
	},
};
```

## Notes

- Noura records the microphone and the system audio as two channels and tells no voices apart within one.
- A model that doesn't stream shows text in chunks, a few seconds behind, as Noura cuts the audio where you pause.
- A device that changes mid-recording, such as headphones you unplug, doesn't stop the recording: Noura moves to the computer's default.
- The plugin hears about a recording that changes state, as when it finishes after `stop`, through a [kernel event](kernel-events.md).
- The mock host has no microphone and no model, so `start` fails there.
