# Read the time

The `clock` interface gives the instant now and this device's time zone. Import it from `noura:plugin/clock@0.1.0`. It needs no capability. It's a plugin's only clock: Noura links no other, and reading the time another way, such as with `Date.now()`, fails the call.

## Functions

- `now()`: a reading with `instant`, the instant now in RFC 3339 and in UTC, such as `2026-10-21T13:30:00Z`, and `timeZone`, the device's IANA time zone, such as `America/Toronto`. A device whose zone has no IANA name gives its offset from UTC, such as `-04:00`.

## Example

This plugin adds a page that shows the time Noura reads and the device's time zone. Its manifest declares `views`:

```toml
id = "example-clock"
name = "Clock"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "views"
```

`render` reads the clock each time Noura draws the page:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import { now } from 'noura:plugin/clock@0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

export const view: ViewExports = {
	pages: () => [{ id: 'clock', title: 'Clock' }],
	render(): Node[] {
		const { instant, timeZone } = now();
		return [
			{
				id: 'root',
				kind: { tag: 'panel', val: 'Clock' },
				children: new Uint32Array([1, 2]),
			},
			{
				id: 'instant',
				kind: { tag: 'text', val: `It's ${instant}.` },
				children: new Uint32Array(),
			},
			{
				id: 'zone',
				kind: { tag: 'text', val: `This device keeps time in ${timeZone}.` },
				children: new Uint32Array(),
			},
		];
	},
};
```

## Notes

- Dates in properties use the workspace format: `YYYY-MM-DD` for a date, and RFC 3339 with an explicit offset for a date and time.
- Pass `clock` to `createMockHost` to fix the time a test reads.
