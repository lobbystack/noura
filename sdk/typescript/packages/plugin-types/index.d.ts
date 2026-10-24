// Noura's plugin interface, noura:plugin@0.1.0, for TypeScript plugins.
//
// The modules `noura:plugin/<interface>@0.1.0` declare what a plugin
// imports: the capability interfaces such as `workspace` and `objects`, and
// the UI types. A plugin's `src/plugin.ts` exports an object for each
// interface it offers Noura, typed with the `...Exports` types below. Each
// function is optional: the build fills in one that returns nothing, so a
// plugin writes only the ones for the capabilities its manifest declares.

/// <reference path="./0.1.0/plugin.d.ts" />

import type * as View from 'noura:plugin/view@0.1.0';
import type * as ObjectTypes from 'noura:plugin/object-types@0.1.0';
import type * as Commands from 'noura:plugin/commands@0.1.0';
import type * as Inbox from 'noura:plugin/inbox@0.1.0';
import type * as Settings from 'noura:plugin/settings@0.1.0';
import type * as Tools from 'noura:plugin/tools@0.1.0';

/** Pages in the rail and their views. Needs the `views` capability. */
export type ViewExports = Partial<typeof View>;

/** Object types and their properties panels. Needs `object-types`. */
export type ObjectTypesExports = Partial<typeof ObjectTypes>;

/** Commands for the command palette and other plugins. Needs `commands`. */
export type CommandsExports = Partial<typeof Commands>;

/** Sections in the Inbox. Needs `inbox-sections`. */
export type InboxExports = Partial<typeof Inbox>;

/** A page in Settings. Needs `settings`. */
export type SettingsExports = Partial<typeof Settings>;

/** Tools for AI assistants. Needs `tools`. */
export type ToolsExports = Partial<typeof Tools>;
