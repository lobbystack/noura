# Handle refused and failed calls

The `types` interface defines `error`, what every capability interface returns when Noura refuses or fails a call. Import its types from `noura:plugin/types@0.1.0`. It has no functions and needs no capability.

## How a call fails

A function whose WIT result can fail returns its value or throws. The thrown error holds the `error` variant in its `payload`, a `{ tag, val }` object:

- `capability-not-declared`: the manifest doesn't declare the capability the call needs, or you revoked it. `val` names the capability, such as `workspace-read`. Noura ran nothing for the call.
- `invalid-path`, `not-found`, `already-exists` and `unsupported-path`: the path or ID the plugin passed. `val` holds it.
- `revision-conflict` and `identity-conflict`: the object's file changed since the plugin read it, or two files hold its ID. Nothing was written.
- `invalid-property`: a property doesn't fit the object type's schema. `val` names the property and the problem.
- `network-refused`: Noura refused a request before connecting.
- `target-not-declared`, `target-unavailable` and `busy`: a call to another plugin's command didn't reach it.
- `failed`: Noura couldn't finish the call. `val` holds a message for you, the plugin's author.

The [reference](../reference/noura-plugin-0.1.0.md) describes each case.

## Example

This plugin adds a page that shows the first line of `Today.md`, or says why it can't. Its manifest declares `workspace-read` and `views`:

```toml
id = "example-types"
name = "Today"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "workspace-read"

[[capabilities]]
name = "views"
```

`explain` reads the payload of the error `readFile` throws and turns each case into a sentence:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import type { Error as NouraError } from 'noura:plugin/types@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';
import { readFile } from 'noura:plugin/workspace@0.1.0';

function explain(error: unknown): string {
	const payload = (error as { payload?: NouraError }).payload;
	switch (payload?.tag) {
		case 'not-found':
			return `${payload.val} isn't in the workspace yet.`;
		case 'capability-not-declared':
			return `plugin.toml doesn't declare ${payload.val}.`;
		case 'failed':
			return payload.val;
		default:
			return `Noura couldn't read the file: ${payload?.tag ?? String(error)}.`;
	}
}

function today(): string {
	try {
		const text = new TextDecoder().decode(readFile('Today.md').bytes);
		return text.split('\n')[0] ?? '';
	} catch (error) {
		return explain(error);
	}
}

export const view: ViewExports = {
	pages: () => [{ id: 'today', title: 'Today' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'panel', val: 'Today' },
			children: new Uint32Array([1]),
		},
		{
			id: 'line',
			kind: { tag: 'text', val: today() },
			children: new Uint32Array(),
		},
	],
};
```

## Notes

- An error the plugin doesn't catch, such as a `TypeError` in your own code, fails the whole call. Noura shows that the plugin failed in place of its view. A plugin that fails three times within a minute stops until you choose Start again in Settings > Plugins.
- The mock host throws the same payloads, so a test can check each case. See the [overview](../README.md#test-it-against-the-mock-host).
