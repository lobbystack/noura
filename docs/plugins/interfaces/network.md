# Send HTTPS requests

The `network` interface sends requests to the HTTPS hosts the manifest lists. Import it from `noura:plugin/network@0.1.0`. It needs the `network` capability with a list of `hosts`, which you approve one by one.

## Functions

- `fetch(request)`: send a request and wait for the answer. A request has a `method` (`get`, `head`, `post`, `put`, `patch` or `delete`), an `https` URL, headers and a body as bytes. The answer has the status, the URL that answered, headers and the body.

Noura refuses a request with `network-refused` before it connects when the URL isn't HTTPS, when its host isn't one the manifest lists, or when the host is a local or private address. It follows up to three redirects, each to a listed host. The time spent waiting for the answer doesn't count toward the call's deadline.

## Example

This plugin offers AI assistants a tool that reads the temperature at a place from Open-Meteo. Its manifest declares `network` with Open-Meteo's API host, and `tools`:

```toml
id = "example-network"
name = "Temperature"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "network"
hosts = ["api.open-meteo.com"]

[[capabilities]]
name = "tools"
```

`callTool` builds the URL from the tool's arguments, sends the request and returns the temperature:

```ts
import type { ToolsExports } from '@noura/plugin-types';
import { fetch } from 'noura:plugin/network@0.1.0';

export const tools: ToolsExports = {
	tools: () => [
		{
			name: 'temperature',
			description:
				'The temperature now, in degrees Celsius, at a latitude and longitude.',
			inputSchema: JSON.stringify({
				type: 'object',
				properties: {
					latitude: { type: 'number' },
					longitude: { type: 'number' },
				},
				required: ['latitude', 'longitude'],
			}),
		},
	],

	callTool(_name: string, args: string): string {
		const { latitude, longitude } = JSON.parse(args) as {
			latitude: number;
			longitude: number;
		};
		const url = `https://api.open-meteo.com/v1/forecast?latitude=${latitude}&longitude=${longitude}&current=temperature_2m`;
		const response = fetch({
			method: 'get',
			url,
			headers: [],
			body: new Uint8Array(),
		});
		if (response.status !== 200) {
			throw `Open-Meteo answered with status ${response.status}.`;
		}
		const answer = JSON.parse(new TextDecoder().decode(response.body)) as {
			current: { temperature_2m: number };
		};
		return JSON.stringify({ celsius: answer.current.temperature_2m });
	},
};
```

## Notes

- A host in the manifest is a name such as `api.example.com`. Noura refuses `localhost`, names ending in `.local` and names without a dot when it installs the plugin.
- The mock host sends no requests. Pass `fetch` to `createMockHost` to answer the ones that pass the manifest's hosts.
