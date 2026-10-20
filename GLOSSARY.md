# Noura

Noura is a local-first workspace for humans and AI. Your work lives in ordinary files, and plugins add the features each workspace needs.

## Language

**Kernel**: The always-on part of Noura that plugins build on: files and the file tree, the editor, search, settings, sync and the plugin system. _Avoid_: core, platform

**Plugin**: A package that adds object types, views, commands or AI tools to a workspace through the public plugin API. Some plugins ship with Noura and others are installed later, but every plugin runs in a sandbox and gets only the capabilities it declares. _Avoid_: module, extension, add-on, built-in plugin

**Capability**: A permission a plugin declares, such as reading the workspace or calling a network host, that you approve when you install the plugin. _Avoid_: permission scope, grant
