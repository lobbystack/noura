// Noura's plugin interface, noura:plugin, for TypeScript plugins.
//
// The modules `noura:plugin/<interface>@<version>` declare what a plugin
// imports: the capability interfaces such as `workspace` and `objects`, and
// the UI types. The package declares them for every interface version the
// kit builds, so a plugin built against an earlier version keeps its types
// when the kit moves on.
//
// What a plugin exports is typed by `@noura/plugin-types/<version>`, such as
// `@noura/plugin-types/0.1.0`, the version its manifest names. It holds a
// `...Exports` type for each interface a plugin offers Noura.

/// <reference path="./generated/versions.d.ts" />
