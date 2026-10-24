# {{name}}

A plugin for [Noura](https://noura.app), written in TypeScript. It adds a page to the rail that counts the files in your workspace and the times you press its button.

## Build and run it

```sh
npm install
npm run dev
```

Dev mode builds `src/plugin.ts` into `dist/plugin.wasm`, then builds it again each time you save. In Noura, open Settings > Plugins, choose Install from folder, and pick this folder. Noura lists the capabilities in `plugin.toml` before it installs anything, and reloads the plugin after each build.

## Test it

```sh
npm test
```

The test runs the built plugin against a mock of Noura, which answers as Noura does and refuses any call that `plugin.toml` doesn't declare.

## Release it

```sh
npm run package
```

This writes the release archive `dist/{{id}}-0.1.0.zip`, which holds `plugin.toml` and the component. Noura compiles the component itself on each device that installs the plugin, so the archive holds no compiled code.

## Files

- `plugin.toml`: the plugin's ID, name, version, the interface version it's built against, and the capabilities it asks for.
- `src/plugin.ts`: the plugin. It imports what it calls from `noura:plugin/...` modules and exports an object for each interface it offers Noura.
- `test/plugin.test.ts`: the test.
