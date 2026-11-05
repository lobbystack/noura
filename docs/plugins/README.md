# Write a Noura plugin in TypeScript

A Noura plugin adds pages, object types, commands, Inbox sections, a Settings page or AI tools to a workspace. It runs as a WebAssembly component in a sandbox, and it reaches files and the network only through the capabilities its manifest declares and you approve. This guide takes you from a new project to a release archive with the TypeScript author kit.

To write a plugin in Rust instead, see [Write a Noura plugin in Rust](rust.md).

The kit is four npm packages:

- `@noura/create-plugin`: the `create-noura-plugin` command, which writes a new project.
- `@noura/plugin-cli`: the `noura-plugin` command, which builds, rebuilds on save and packages a plugin.
- `@noura/plugin-types`: the TypeScript types of Noura's plugin interface, generated from its WIT.
- `@noura/mock-host`: a mock of Noura that runs a built plugin in your tests.

The kit needs Node 22.20 or a later 22 release, 24.12 or a later 24 release, or 26 or later. CI tests it on Node 24.

## Start a plugin

1. Write a new project. The folder's name is the plugin's ID: lowercase letters, digits and hyphens, starting with a letter.

   ```sh
   npx @noura/create-plugin word-count
   ```

2. Install the kit and start dev mode:

   ```sh
   cd word-count
   npm install
   npm run dev
   ```

3. In Noura, open Settings > Plugins, choose Install from folder and pick the `word-count` folder. Noura lists the capabilities the plugin asks for before it runs any of its code. Approve them, and the plugin's page appears in the rail.

The project holds:

- `plugin.toml`: the manifest.
- `src/plugin.ts`: the plugin, a page that counts the files in your workspace and the times you press its button.
- `test/plugin.test.ts`: its test, which runs the built plugin against the mock host.
- `package.json` and `tsconfig.json`, which set up the kit, TypeScript and Vitest.

## Use the kit from this repository

The `@noura` packages aren't on npm yet. To try the kit, build it from a clone of this repository and run its commands from there:

```sh
cd sdk/typescript
npm ci
npm run build
node packages/create-plugin/dist/cli.js ~/word-count
node packages/plugin-cli/dist/cli.js dev ~/word-count
```

To install the packages into a project instead, run `npm pack --workspaces` in `sdk/typescript` and `npm install` the four `.tgz` files it writes.

## Declare what the plugin needs

`plugin.toml` names the plugin and lists its capabilities:

```toml
id = "word-count"
name = "Word count"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "workspace-read"

[[capabilities]]
name = "views"
```

- `id`: lowercase letters, digits and hyphens, starting with a letter, up to 64 characters.
- `version`: the plugin's semantic version.
- `interface-version`: the version of Noura's plugin interface, `noura:plugin`, that the plugin is built against. The build reads the interface of this version.
- `entry`: the path the build writes the component to. It ends in `.wasm` and sits inside the plugin's folder but outside `src`, such as `dist/plugin.wasm`. The build won't replace a file there that isn't a component.
- `platforms`: `desktop`, and `mobile` once a mobile Noura exists.

Each `[[capabilities]]` table asks for one capability:

| Capability | Lets the plugin | Interfaces |
| --- | --- | --- |
| `workspace-read` | Read the workspace's files, folders and objects | [`workspace`](interfaces/workspace.md) |
| `object-types` | Add its own object types with a properties panel, and create, change, move and delete their objects | [`object-types`](interfaces/object-types.md), [`objects`](interfaces/objects.md) |
| `views` | Add pages to the rail, and open pages and objects in answer to something you did | [`view`](interfaces/view.md), [`navigation`](interfaces/navigation.md) |
| `network` with `hosts` | Send HTTPS requests to the hosts it lists | [`network`](interfaces/network.md) |
| `commands`, optionally with `targets` | Offer commands to the palette and other plugins, and run the commands its targets name | [`commands`](interfaces/commands.md), [`command-calls`](interfaces/command-calls.md) |
| `inbox-sections` | Add sections to the Inbox | [`inbox`](interfaces/inbox.md) |
| `settings` | Add a page to Settings | [`settings`](interfaces/settings.md) |
| `tools` | Offer tools to AI assistants | [`tools`](interfaces/tools.md) |
| `ai` | Chat with the AI providers you set up once you consent for the workspace, and keep the chats in it | [`ai`](interfaces/ai.md) |

A call the manifest doesn't cover fails with `capability-not-declared`, naming the capability, and Noura runs nothing for it. The [`clock`](interfaces/clock.md) needs no capability. You approve each capability, network host and command target on each device, and you can revoke any of them in Settings > Plugins. An update that asks for more waits for your approval.

## Write the plugin

`src/plugin.ts` imports what it calls from the modules `noura:plugin/<interface>@0.1.0`, and exports an object for each interface it offers Noura: `view`, `objectTypes`, `commands`, `inbox`, `settings` and `tools`, typed with `@noura/plugin-types/0.1.0`. Each function is optional, since the build fills in one that returns nothing:

```ts
import type { ViewExports } from '@noura/plugin-types/0.1.0';
import type { Node } from 'noura:plugin/ui@0.1.0';

export const view: ViewExports = {
	pages: () => [{ id: 'hello', title: 'Hello' }],
	render: (): Node[] => [
		{
			id: 'root',
			kind: { tag: 'text', val: 'Hello from a plugin.' },
			children: new Uint32Array(),
		},
	],
};
```

Each interface has a page with an example that CI builds and runs in Noura:

- Imports: [`workspace`](interfaces/workspace.md), [`objects`](interfaces/objects.md), [`network`](interfaces/network.md), [`command-calls`](interfaces/command-calls.md), [`clock`](interfaces/clock.md), [`navigation`](interfaces/navigation.md) and [`ai`](interfaces/ai.md).
- Exports: [`view`](interfaces/view.md), [`object-types`](interfaces/object-types.md), [`commands`](interfaces/commands.md), [`inbox`](interfaces/inbox.md), [`settings`](interfaces/settings.md) and [`tools`](interfaces/tools.md).
- Types: [`ui`](interfaces/ui.md), the nodes and events of views, and [`types`](interfaces/types.md), the errors calls throw.

The [interface reference](reference/noura-plugin-0.1.0.md) lists every type and function, from the WIT's own comments.

A plugin runs with limits:

- Each call may run for 1 second of the plugin's own work. Time spent waiting for the network or another plugin doesn't count, up to 2 minutes in all.
- A plugin may hold 256 MB of memory.
- A plugin has no file system, sockets or console. What it writes to `console` goes nowhere, and reading the time other than through `clock` fails the call.

A call that runs past a limit or throws fails on its own, and Noura starts the plugin again for its next call. A plugin that fails three times within a minute stops until you choose Start again in Settings > Plugins.

## Rebuild on save

`npm run dev` runs `noura-plugin dev`. It builds `src/plugin.ts` into the component that `entry` names, then builds again each time you save a file in `src` or `plugin.toml`. Noura watches the folder you installed from and reloads the plugin after each build, keeping its open pages. A build that fails prints why and leaves the last good component in place.

A new build that asks for a capability you haven't approved waits in Settings > Plugins until you approve it.

## Test it against the mock host

`npm test` builds the plugin and runs its tests with Vitest. A test starts the mock host on the built plugin, gives it a workspace and drives it as Noura does:

```ts
import { createMockHost } from '@noura/mock-host';
import { expect, test } from 'vitest';

test('the page counts the files', async () => {
	const host = await createMockHost({ files: { 'Plan.md': 'plan' } });
	expect(await host.texts('main')).toContain('Your workspace holds 1 file.');
	await host.press('main', 'press');
	await host.close();
});
```

The mock host implements every interface of `noura:plugin` from the same WIT the plugin builds against, and refuses each call the manifest doesn't declare with the error Noura gives. Its options fill in what it can't reach:

- `files`: the workspace's files, by path. A Markdown file with an `id` and a `type` in its frontmatter is an object, and `workspace` reads it as Noura does. The `objects` interface sees only the objects the plugin writes, and `readFile` gives their files with the frontmatter Noura writes.
- `clock`: what `now()` reads.
- `fetch`: answers to the requests that pass the manifest's hosts.
- `commands`: other plugins' commands, by `plugin/command`.
- `providers`: the AI providers that `ai` lists.

The host's methods drive each export:

- Pages: `pages`, `view`, `texts`, `sidebar`, `rows`, `send` and `press`.
- Commands: `commands`, `runCommand`, as the palette runs one, and `callCommand`, as another plugin calls one.
- The Inbox and Settings: `inboxSections`, `inboxSection`, `sendInbox`, `settingsPage`, `settings` and `sendSettings`.
- Tools: `tools` and `callTool`.
- Object types: `objectTypes`, `panel` and `sendPanel`.

`host.objects()` lists the objects the plugin wrote, and `host.openings` what it asked Noura to open. Without its capability, a listing method such as `pages` returns nothing, since Noura never asks, and the other methods throw a `CapabilityError` that names the capability.

The mock host doesn't check views against Noura's drawing rules or properties against their schemas, and it doesn't fill in a schema's defaults. It asks no AI provider and nobody for consent, so an `ai` run waits for consent until the plugin stops it. Run the plugin in Noura for those.

## Package it for release

`npm run package` runs `noura-plugin package`. It builds the plugin, then writes its release archive next to the component, `dist/<id>-<version>.noura-plugin.zip` for the template.

The release archive is a zip file that holds:

- `plugin.toml` at its root.
- The component, at the path the manifest's `entry` names.

To publish the plugin, attach the archive to a release on GitHub. In Noura, you paste the release's address under Install from GitHub in Settings > Plugins. Noura downloads the one file of the release whose name ends in `.noura-plugin.zip`, up to 10 MB, and lists what the plugin asks for before it installs anything.

Noura reads only those two files and installs nothing from an archive that holds a link, a path that leaves the plugin's folder or two entries with one path. It compiles the component itself on each device, and refuses a component compiled elsewhere, so the archive holds no compiled code.

## Interface versions

The interface's version is part of every module name, such as `noura:plugin/workspace@0.1.0`, and the manifest's `interface-version` names it. When Noura ships a new version of the interface, it keeps running plugins built against the older ones, and the kit keeps building them. `@noura/plugin-types` keeps the types of each version too. Import your plugin's export types from `@noura/plugin-types/<version>`, the version its manifest names, and a kit update leaves them as they were. CI builds a plugin against `noura:plugin@0.1.0` and runs it on every change, so a release that would break it fails first.
