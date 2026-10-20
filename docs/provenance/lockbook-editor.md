# Lockbook Markdown editor

Noura's editor starts from Lockbook's live-preview Markdown editor. This record covers every file taken from Lockbook, what changed, and what stayed behind.

## Source

- Repository: https://github.com/lockbook/lockbook
- Commit: `1cd633a9a1feb09d4c02d21895fc9ec6c674c08d` (2026-10-20, "lb-rs: fix IPC for long paths (#5210)")
- License: the Unlicense, from the repository's root `UNLICENSE`. The `workspace` crate's manifest declares no other license.
- Taken from: `libs/content/workspace/src/` at that commit.

`libs/lb/lb-rs` declares `license = "BSD-3-Clause"` in its own manifest, which differs from the root Unlicense. Noura copies no `lb-rs` source. `editor-core` reimplements the interface the editor used from `lb-rs` (the `Byte`, `Grapheme`, `Bytes` and `Graphemes` offsets, their range helpers, and the text buffer) in new code over ropey.

## Destination

- `crates/editor-core`: the text side of the editor. It holds the buffer, offsets, comrak parsing, bounds, reveal rules, folding rules and edit commands, and depends on no UI crate.
- `crates/editor-egui`: layout, painting, scrolling and input translation on egui and eframe 0.37, with text shaped by glyphon and painted through eframe's wgpu renderer.

## Files taken

Paths are relative to `libs/content/workspace/src/`.

| Source | Destination | Notes |
|---|---|---|
| `tab/markdown_editor/` (the editor folder) | `crates/editor-egui/src/`, with the text code moved to `crates/editor-core/src/` | Excluding the files listed under "Left behind" |
| `widgets/affine_scroll.rs` | `crates/editor-egui/src/widgets/affine_scroll.rs` | The virtualized scroll area |
| `widgets/glyphon_render.rs`, `widgets/glyphon_cache.rs` | `crates/editor-egui/src/widgets/` | The glyphon paint callback and shaped-text cache |
| `widgets/caret.rs` | `crates/editor-egui/src/widgets/caret.rs` | Caret painting and blink |
| `resolvers/` (the trait shapes) | `crates/editor-core/src/resolvers.rs` | Rewritten as interfaces the embedding app implements |
| `theme/palette_v2.rs`, and the colors and metrics the editor reads from `style/` | `crates/editor-egui/src/theme/`, `crates/editor-egui/src/style.rs` | Cut down to what the editor reads |
| `seq.rs` | `crates/editor-core/src/seq.rs` | The sequence counter, no longer stored in egui's context |

## Modifications

- Ported from egui 0.32.3 to egui and eframe 0.37, glyphon 0.9 to 0.12, and cosmic-text 0.14 to 0.19, so the build holds the single wgpu version eframe uses.
- Replaced the `lb-rs` buffer and offsets with `editor-core`'s ropey buffer and offsets.
- Split the code between `editor-core` (text decisions) and `editor-egui` (layout, painting and input).
- Replaced `lb-rs` link, embed and image lookups with interfaces the embedding app implements.
- Removed every network request: link cards, link metadata fetching and remote image fetching.
- Removed code-block syntax highlighting and its syntect dependency. Ticket 18 picks the highlighter and checks its grammar licenses.
- Removed Lockbook's fonts. The embedding app supplies fonts to `editor-egui`.
- Removed the mobile host code and the platform command surface for iOS and Android.

Lockbook's editing behaviors stay as they were, including the ones that write files. Ticket 18 removes those.

## Left behind

| Source | Reason |
|---|---|
| `lb-fonts` (a git dependency) | It has no license and bundles Apple's SF Pro and SF Mono fonts |
| `libs/content/egui_wgpu_renderer` | Lockbook's own wgpu host for mobile. eframe's renderer replaces it |
| `tab/markdown_editor/assets/placeholders.tmTheme` | Syntax highlighting stays out until ticket 18 |
| `tab/markdown_editor/widget/inline/link/card.rs`, `widget/inline/link/meta.rs` | Link cards fetch web pages |
| `tab/markdown_editor/commands.rs` | Platform commands for the iOS and Android hosts |
| `tab/markdown_editor/tests/ios_host.rs` | Tests the iOS host |
| `tab/markdown_editor/tests/link_card.rs` | Tests link cards |

## Notices

The Unlicense dedicates the code to the public domain and asks for no notice. Noura keeps this record so you can trace each file back to its source.

comrak enters the product with this fork. `THIRD_PARTY_NOTICES.md` carries its BSD-2-Clause notice.

## Review

- Reviewer: Claude (author review). Owner review pending.
- Date: 2026-10-20.
