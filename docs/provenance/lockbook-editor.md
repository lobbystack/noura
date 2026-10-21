# Lockbook Markdown editor

Noura's editor starts from Lockbook's live-preview Markdown editor and the text model it edits through. This record lists every file taken from Lockbook, what changed, what stayed behind, and which of Lockbook's tests didn't come along.

## Sources

Both sources come from one repository at one commit:

- Repository: https://github.com/lockbook/lockbook
- Commit: `1cd633a9a1feb09d4c02d21895fc9ec6c674c08d` (2026-10-20, "lb-rs: fix IPC for long paths (#5210)")

The editor comes from the `workspace` crate, `libs/content/workspace/src/`. The repository's root `UNLICENSE` dedicates it to the public domain under the Unlicense, and the crate's manifest names no other license.

The text buffer, offsets and diff come from the `lb-rs` crate's text model, `libs/lb/lb-rs/src/model/text/`. The `lb-rs` manifest declares `license = "BSD-3-Clause"`, which overrides the root Unlicense for that crate, so Noura treats these files as BSD-3-Clause code. `lb-rs` ships no license file naming a copyright holder. Noura's notice credits "the Lockbook authors" and dates it from these files' history, 2023 to 2026.

The first version of this record said Noura copied no `lb-rs` source and that `editor-core` reimplemented the text model in new code. That was wrong: `editor-core`'s buffer, diff, offsets and grapheme index follow the `lb-rs` files function by function. The record and `THIRD_PARTY_NOTICES.md` now treat them as adapted BSD-3-Clause code.

## Destination

- `crates/editor-core`: the text side. It owns the buffer, offsets, comrak parsing, bounds, node ranges, block prefixes, reveal rules, folding rules, image and link atoms, list reordering and the edit commands, and depends on no UI crate.
- `crates/editor-egui`: layout, painting, scrolling and input translation on egui and eframe 0.37. It shapes text with glyphon and paints it with a paint callback through eframe's own egui-wgpu renderer.

## Files taken from the workspace crate (Unlicense)

Paths on the left are relative to `libs/content/workspace/src/`.

| Source | Destination |
|---|---|
| `tab/markdown_editor/mod.rs` | `crates/editor-egui/src/lib.rs`, with the parse and sequence logic in `crates/editor-core/src/doc.rs` |
| `tab/markdown_editor/bounds.rs` | `crates/editor-core/src/bounds.rs` |
| `tab/markdown_editor/fold.rs` | `crates/editor-core/src/fold.rs` (rules) and `crates/editor-egui/src/fold.rs` (chip drawing and clicks) |
| `tab/markdown_editor/text_units.rs` | `crates/editor-core/src/text_units.rs` |
| `tab/markdown_editor/input/mod.rs`, `advance.rs`, `mutation.rs` | `crates/editor-core/src/input/`, with pointer hit-testing and row navigation in `crates/editor-egui/src/input/layout.rs` |
| `tab/markdown_editor/input/canonical.rs`, `cursor.rs` | `crates/editor-egui/src/input/`, with the paste rules in `crates/editor-core/src/input/mutation.rs` |
| `tab/markdown_editor/show.rs`, `scroll_content.rs`, `theme.rs` | `crates/editor-egui/src/` |
| `tab/markdown_editor/widget/` (except the files under "Left behind") | `crates/editor-egui/src/widget/`. The text logic inside these files moved to `crates/editor-core/src/nodes.rs`, `blocks.rs`, `reveal.rs`, `images.rs`, `links.rs` and `reorder.rs`, and the task checkbox's edit to `input/mutation.rs` |
| `tab/markdown_editor/tests/` (except the files under "Left behind") | `crates/editor-egui/src/tests/`, with test modules for moved code in the matching `editor-core` files |
| `widgets/affine_scroll.rs`, `glyphon_render.rs`, `glyphon_cache.rs`, `caret.rs` | `crates/editor-egui/src/widgets/` |
| `theme/palette_v2.rs` | `crates/editor-egui/src/palette.rs` |
| `style/overlay_scroll.rs`, and the tokens the editor reads from `style/chrome.rs`, `color.rs`, `space.rs`, `typography.rs` and `interact.rs` | `crates/editor-egui/src/style/` |
| `seq.rs` | `crates/editor-egui/src/seq.rs` |
| `resolvers/embed.rs` (the `EmbedResolver` trait) | `crates/editor-egui/src/resolvers.rs` |
| `resolvers/link.rs` (the `LinkResolver` trait and `LinkState`) | `crates/editor-core/src/resolvers.rs`, rewritten for the embedding app |
| `test_utils/prop.rs`, `byte_source.rs`, `shrink.rs` | `crates/editor-egui/src/tests/test_utils/` |
| `tab/markdown_editor/widget/find.rs` (added by ticket 19) | `crates/editor-core/src/find.rs` (matches, the current match, stepping and replacing) and the find bar in `crates/editor-egui/src/chrome.rs` |

## Files adapted from lb-rs (BSD-3-Clause)

Paths on the left are relative to `libs/lb/lb-rs/src/model/text/`. Each destination file names its source in its module comment.

| Source | Destination |
|---|---|
| `buffer.rs` | `crates/editor-core/src/buffer.rs`, with its tests in `crates/editor-core/tests/history.rs` |
| `mod.rs` (the word diff `diff`) | `crates/editor-core/src/diff.rs` |
| `offset_types.rs` | `crates/editor-core/src/offset_types.rs` |
| `operation_types.rs` | `crates/editor-core/src/operation_types.rs` |
| `unicode_segs.rs`, and `measure_replace` and `last_grapheme` from `units.rs` | `crates/editor-core/src/unicode_segs.rs` |

## Modifications

To the workspace crate's editor:

- Ported from egui 0.32.3 to egui and eframe 0.37 (`Id::unique`, `Ui::scope_id`, `Context::run_ui`, the new IME output fields, `fonts_mut`) and to Rust edition 2024.
- Ported from glyphon 0.9 to 0.12 and cosmic-text 0.14 to 0.19, which shape on an explicit `shape_until_scroll` call. glyphon 0.12 targets wgpu 30, the version eframe 0.37 uses, so the build holds one wgpu version.
- Split the editor in two. `editor-core`'s `MdDoc` holds the buffer, the parse, the reveal state and the edit commands. `editor-egui`'s renderer holds an `MdDoc` and derefs to it. Edit commands ask layout questions (where a click lands, which offset sits a row away) through a `TextLayout` trait that `editor-egui` answers from the fragments it painted. With no layout, Up and Down step over source lines and hold a column counted in graphemes.
- Moved three edits that `editor-egui` decided into `editor-core` events: `Paste`, `ToggleTask` (the checkbox writes `x` or a space) and `Unfold` (the fold chip removes its tag). Ticket 19 made a paste replace the selection with the pasted text as copied, with Windows line endings turned into `\n`. Lockbook turned non-breaking spaces into spaces and linked selected text to a pasted URL.
- Ticket 19 replaced Lockbook's formatting commands, Tab handling and shortcuts with the legacy app's rules, ported from `packages/editor/src/commands.ts` into `crates/editor-core/src/commands.rs`, and a keymap in `crates/editor-core/src/keymap.rs` that `editor-egui` runs. Tab nests a list item with its children under the item above and Shift-Tab lifts them out; outside a list Tab moves the keyboard to the next control. Mod-Enter checks a task instead of opening the links in the selection. Ticket 19 also removed Lockbook's shortcuts for highlight, spoilers, subscript, superscript, code blocks, thematic breaks and headings 4 to 6. The code block and thematic break shortcuts reached `toggle_style` paths that panic.
- Ticket 19 adapted Lockbook's find widget. Matching, the current match, stepping and replacing moved to `editor-core`, which matches literal text ignoring case through `regex`. The bar uses egui's own text fields and buttons. Lockbook's case, whole-word and regex modes stayed behind.
- Replaced `lb-rs` link and image lookups with the `LinkResolver` and `EmbedResolver` interfaces the embedding app implements. Opening a link reports an `OpenedLink` to the app instead of opening it. A wikilink to a missing note reports not found and creates nothing. `EmbedResolver` answers with an `EmbedState` that can say not found, and the editor draws a broken-image marker for it. The `()` resolver finds nothing. Both traits require `Send`, so an editor can sit in a window that egui draws as a deferred viewport.
- Removed every network request: link cards, link metadata fetching, and Lockbook's image cache, which read files through `lb-rs` and fetched remote images.
- Removed code-block and plain-text syntax highlighting with its syntect and syntect-assets dependencies. Ticket 18 highlights fenced code with arborium instead, in new code.
- Removed Lockbook's fonts. `register_font_system` takes a font database from the embedding app, and `fonts::default_font_database` offers egui's bundled fonts. Phosphor and Nerd Font icon glyphs became painted shapes (the heading fold caret, the alert dot) or text (the `…` fold chip), and emoji no longer route to Twemoji.
- Removed the editor's chrome: the toolbar, find bar, emoji and link completions, context menus, link hover cards and link sheets. Removed the mobile host command surface, `MdLabel`, and the workspace persistence of scroll position and selection.
- The palette falls back to Lockbook's default light or dark theme when the app sets none.
- `alert_type_title_ranges` gives a callout whose first line names no known type an empty type and uses the whole line as its title. Lockbook panicked there. You can reach that case by typing `---` above a line that holds a lone carriage return, which makes frontmatter that comrak counts one line short.
- `own_prefix_len_task_item` keeps a task item's prefix inside its line. A parent item's padding counts columns, so after a tab it can claim part of the task's marker, as in `-\t- [ ] `. Lockbook then read past the end of the line and panicked.
- Spoilers remember their reveal by source range instead of by egui id.
- Copied and cut text leaves `editor-core` through `MdDoc::copied`, and `editor-egui` hands it to egui's clipboard.
- Inserts an input method's committed text the way a key press inserts text. Lockbook's desktop path ignored egui's input method events.

To the `lb-rs` text model:

- The buffer keeps its text in a ropey rope instead of a `String`. Edits, undo, redo and merged outside edits write the rope, and the save text comes from it. A contiguous copy, rebuilt from the rope after each change, feeds comrak and `&str` slices.
- Rewrote the operation history as two lists, received and applied, in place of `Ops`, and dropped the base snapshot, `Buffer::merge` and the `open_camera` response flag. `queue_at` takes the time of an edit, so tests set keystroke times instead of sleeping.
- `lb-rs` merged a reload by diffing the file against the last text loaded or saved, then applying the diff at the version that held that text. Undo past that version, followed by any edit or reload, drops the version from the history. A merge into unsaved edits leaves no version holding the file's text. In both cases `lb-rs` applied the diff to the wrong text and mangled the note. Noura's buffer merges on its next update instead. While a version still holds the last text loaded or saved, it transforms the diff through the edits made since. Otherwise it merges three texts: that one, the buffer's and the file's. Offsets that still run past the text clamp to it instead of panicking.
- Undo takes back your own edits only. `lb-rs` undid a merged outside edit as it undid typing. When an outside edit arrived after your last unit, Noura's undo first rewrites that stretch of the history so the outside edit comes before your unit. It then takes your unit back. Where the outside edit changed text your unit wrote, the outside edit stays.
- `lb-rs` moved both ends of a replacement past text an earlier edit inserted there. Merging another app's edit of a word then deleted a keystroke you typed right after it. Noura keeps a replacement's end before such an insertion, and `buffer_merge_insert_replace` now expects your keystroke to stay.
- Restructured the word diff around a `Side` helper with the same algorithm, and took only `measure_replace` and `last_grapheme` from `units.rs`.
- Uses `std::time` in place of `web_time`, and renamed identifiers throughout.

Ticket 18 changed the fork so notes from Obsidian-style workspaces read as they do in today's editor:

- Nothing folds. `foldable` answers `None`, so `ToggleFold` and `Unfold` change no text, and a fold marker already in a file shows as written. The code that reads fold markers stays, inert.
- A paste keeps its non-breaking spaces.
- comrak reads setext headings, and reads `__text__` as strong. Its alerts, spoilers, emoji shortcodes, superscript, subscript, code math and front matter extension are off, so their text shows as written. comrak reads `~x~` as strikethrough and has no option to stop, so the editor draws single-tilde strikethrough as written.
- HTML the editor doesn't draw shows as written. Lockbook hid it away from the caret.
- A soft line break keeps its own row, as Lockbook drew it.
- `lookahead.rs` reads frontmatter and `$$` math within a 256 KiB lookahead. comrak parses a copy of the note in which the frontmatter's delimiter lines read as code fences, byte for byte, so source positions after frontmatter that holds a lone carriage return line up again. In that copy, a line inside a `$$` block that looks like block syntax starts with a stand-in byte, so the block stays one piece of math.
- The `ToggleStyle` event for underline wraps the selection in `<u>` and `</u>`, or takes them off, as ticket 19's Mod-U command does. It used to write `__`, which now draws as strong.
- New code reads callouts of any type (`callouts.rs`), and comments, tags, block IDs, `<u>`, embeds and image embeds (`spans.rs`). The alert widget no longer draws anything, because comrak's alerts are off.
- A table shows its raw rows while the caret is anywhere in it, and each grid cell is a cell in the accessibility tree.
- Display math hides its `$$` instead of one `$`, and math draws in a format of its own.
- Remote images draw a Load image button, and a drawn image is an image in the accessibility tree.
- A setext heading's underline line draws its quote or list prefix while it shows. Lockbook drew none, which the `cursor_renders` property found once setext headings came on.
- `PaintedSpan` reports the text a fragment drew and whether it drew its source as written.

Ticket 19 removed Lockbook's Tab handling and the flattening of non-breaking spaces on paste.

## Left behind

| Source | Reason |
|---|---|
| `lb-fonts` (a git dependency) | It has no license and bundles Apple's SF Pro and SF Mono fonts |
| `libs/content/egui_wgpu_renderer` | Lockbook's own wgpu host for mobile. eframe's renderer replaces it |
| `libs/lb/lb-rs`, except the text model files above | Lockbook's core library: accounts, sync, encryption and the file cache |
| `lb-rs/src/model/text/units.rs`, except `measure_replace` and `last_grapheme` | Codepoint units and strict byte conversions the editor doesn't call |
| `tab/markdown_editor/assets/placeholders.tmTheme` | Syntax highlighting stays out until ticket 18 |
| `tab/markdown_editor/widget/inline/link/card.rs`, `widget/inline/link/meta.rs` | Link cards fetch web pages |
| `tab/markdown_editor/widget/toolbar.rs`, `widget/emoji_completions.rs`, `widget/link_completions.rs` | Chrome around the editor that later tickets rebuild for Noura. The toolbar is the phone toolbar, with Lockbook's actions and its style tokens; Noura's desktop toolbar in `crates/editor-egui/src/chrome.rs` is new code laid out like the legacy app's. Link completions also listed files through `lb-rs`, and belong to ticket 20 |
| `tab/markdown_editor/commands.rs` | Platform commands for the iOS and Android hosts |
| `tab/markdown_editor/input/events.rs` | `MdEdit::drain_workspace_events` took Markdown edits, Undo and Redo from Lockbook's workspace event queue. The fork has no such queue: the embedding app passes events to `Editor::push_event` |
| `tab/markdown_editor/md_label.rs` | A read-only Markdown label for Lockbook's chat and search previews |
| `tab/markdown_editor/docs/`, `README.md` | Design notes, linked from here instead |
| `widgets/image_cache.rs`, `resolvers/image_embed.rs`, `file_cache.rs`, `egress.rs` | Read files through `lb-rs` and fetched remote images and pages |
| `resolvers/link.rs`, except the trait and `LinkState` | Lockbook's file-cache link resolver and its share warnings, which read `lb-rs`'s file cache |
| `style/interact.rs`, `chrome.rs` and `color.rs`, except the tokens the editor reads | Press and hover animation, plates, shortcuts, file icons and hue washes for Lockbook's chrome |
| `widgets/glyphon_label.rs`, `widgets/glyphon_text_edit.rs` | Used only by the chrome that stayed behind |

## Tests

### Editor tests

The workspace crate's files that Noura took hold 218 tests: 208 in `tab/markdown_editor/` and `theme/palette_v2.rs`, and 10 in `resolvers/link.rs` and `style/`. 166 came along, 138 in `editor-egui` and 28 in `editor-core`, ported to the new buffer and egui 0.37. 159 run and pass. The 7 benches in `tests/benches.rs` stay `#[ignore]`, as they are upstream.

Five tests changed to fit the fork. `touch_link_tap_selects_and_pops_menu` and `touch_image_tap_selects_and_pops_menu` became `touch_link_tap_selects_link` and `touch_image_tap_selects_image`: the platform menu is gone, so they check the selection and that nothing opens. `caret_bordering_embeds_spans_their_height` keeps its image case and drops its card case. `inline_image_advance_matches_rendered_width` and `image_load_layout_consistent` complete image loads through the test embed resolver instead of `lb-rs`'s image cache.

These 52 didn't come along:

| Test | Reason |
|---|---|
| `tests/link_card.rs`: all 15 tests (`angle_bracket_autolink_is_not_a_card`, `bare_url_alone_on_a_line_is_a_card`, `bare_url_in_a_sentence_is_not_a_card`, `bare_url_under_a_heading_is_a_card`, `cached_metadata_renders_a_card`, `capsule_selection_rects_hug_pill`, `enter_atom_reveals_link_card_source`, `labeled_link_is_not_a_card`, `previews_off_does_not_fetch_autolink_titles`, `rate_limited_meta_degrades_and_gates_refetch`, `setting_mirrors_onto_renderer_each_frame`, `two_urls_on_one_line_are_not_cards`, `url_in_a_container_block_is_not_a_card`, `url_in_list_renders_no_card_even_when_cached`, `wrapped_capsule_edge_carets`) | Link cards and capsules came out with network fetching |
| `widget/inline/link/meta.rs`: `description_absent_is_none`, `falls_back_to_html_title`, `favicon_falls_back_to_origin_root`, `full_card_metadata`, `no_title_yields_none`, `prefers_open_graph_over_html_title`, `real_titles_are_not_junk`, `site_name_falls_back_to_host_without_www`, `twitter_card_fallbacks`, `wall_titles_and_markers_are_junk` | They test scraping fetched pages for link cards |
| `widget/emoji_completions.rs`: `colon_after_space_or_start_does_trigger`, `colon_after_word_does_not_trigger`, `empty_note_has_no_in_doc_recents`, `in_doc_recents_first_appearance_and_cap`, `parse_shortcode_token` | The emoji completion popup stayed behind |
| `widget/link_completions.rs`: `destination_completion_replaces_link_keeping_title`, `destination_context_detected_and_title_kept`, `list_markers_hold_the_popup`, `out_of_scope_links_insert_the_external_url`, `path_query_matches_folder_and_leaf`, `wikilink_completions_resolve_back` | Link completions stayed behind; they need `lb-rs`'s file cache |
| `tests/regressions.rs`: `find_esc_closes_widget` | It tested Lockbook's find bar; `crates/editor-egui/tests/editing_commands.rs` checks that Escape closes Noura's |
| `tests/regressions.rs`: `icon_glyphs_skip_emoji_font` | The icon fonts and Twemoji routing came out with `lb-fonts` |
| `tests/regressions.rs`: `layout_cache_consistent_under_link_title` | Link titles came from fetched metadata |
| `tests/regressions.rs`: `out_of_scope_embed_leaves_image_cache_alone` | It tests `lb-rs`'s image cache and share scope |
| `tests/regressions.rs`: `phone_toolbar_never_pushed_off_screen` | The phone toolbar stayed behind |
| `tests/regressions.rs`: `wrapped_capsule_touch_region_matches_pill` | Link capsules came out with network fetching |
| `resolvers/link.rs`: `states_say_why`, `filled_in_parts_are_bold`, `a_tie_says_so` | They test the file-cache link resolver's share warnings, which stayed behind, and build their files with `lb-rs`'s file cache |
| `style/interact.rs`: `desired_priority`, `retarget_uses_current_as_from`, `same_target_does_not_restart`, `rapid_press_release_always_from_current`, `click_frame_mid_ease_out_snaps_to_press` | The press and hover animation they test stayed behind |
| `style/chrome.rs`: `fit_outside_stroke_pulls_fill_off_clip_edges` | `fit_outside_stroke_fill` stayed behind with the chrome |
| `style/color.rs`: `hue_wash_differs_from_poles` | `hue_wash` stayed behind with the chrome |

Ticket 18 removed 28 more ported tests, because they test folding, which Noura no longer does:

| Test | Reason |
|---|---|
| `tests/folding.rs`: all 25 tests (`fold_tag_never_renders_as_source`, `find_match_reveals_folded_contents_without_unfolding`, `newline_right_of_chip_creates_heading_after_folded_content`, `newline_right_of_chip_creates_item_after_folded_subtree`, `arrow_right_skips_chip_then_skips_section`, `arrow_right_at_doc_end_section_stays_put`, `arrow_left_skips_section_then_chip`, `blank_line_after_folded_section_is_walkable`, `blank_line_after_folded_item_is_walkable`, `trailing_blank_line_after_doc_end_fold_is_reachable`, `backspace_after_folded_section_unfolds_instead_of_joining`, `deleting_selection_of_folded_items_deletes_their_contents`, `cut_selection_with_capsule_takes_contents`, `typing_over_selection_replaces_folded_contents`, `deleting_selection_covering_contents_takes_tag_too`, `backspace_empty_line_after_folded_section_deletes_line`, `forward_delete_right_of_chip_unfolds_instead_of_deleting`, `backspace_right_of_chip_removes_whole_tag`, `click_chip_unfolds`, `selection_into_contents_unfolds_but_select_all_does_not`, `cursor_into_nested_folds_unfolds_all`, `cursor_beside_chip_renders_outside_capsule`, `toggle_fold_round_trip`, `fold_with_selection_into_contents_stays_folded`, `word_select_inside_tag_snaps_to_whole_tag`) | Folding wrote markers into notes |
| `tests/ios_host.rs`: `platform_typing_inside_a_fold_unfolds_it`, `tap_right_of_a_fold_chip_lands_after_it` | The same |
| `tests/regressions.rs`: `down_arrow_into_folded_tail_stops_at_visible_end` | The same |

`render_props::drag_never_reveals` now leaves callouts out along with tables: a click anywhere in a callout shows its source. `editor-core`'s `commands.rs` lost `pasting_flattens_non_breaking_spaces` and `unfolding_removes_the_fold_tag`, which pinned the behaviors ticket 18 removed.

### lb-rs tests

The five `lb-rs` files hold 32 tests, and 30 came along:

- `buffer.rs`: the selection, undo-unit and merge tests, in `crates/editor-core/tests/history.rs`. `Buffer::merge` stayed behind, so the merge tests type one version into the editor and load the other from disk, which queues the same operations against the same base. `typing_long_pause_splits` gives its keystrokes times instead of sleeping.
- `mod.rs`: `diff_full_replace`, `diff_partial_replace` and `diff_word_boundary_inside_grapheme` became `replaces_everything_when_nothing_matches`, `replaces_the_changed_word` and `a_word_boundary_inside_a_cluster_still_yields_valid_ranges` in `diff.rs`. `diff_fuzz` became `random_texts_diff_and_apply_to_the_target`, which also applies each diff.
- `offset_types.rs`: all 3, in `offset_types.rs`.

These 2 didn't come along:

| Test | Reason |
|---|---|
| `buffer.rs`: `buffer_merge_fuzz_chain_2`, `buffer_merge_fuzz_chain_5` | They check that Lockbook's file sync converges through `Buffer::merge` across chains of devices. That merge stayed behind, and Noura's sync merges through yrs |

### Tests removed by ticket 19

Ticket 19 removed seven ported tests that checked Lockbook's Tab handling, which the legacy app's list nesting replaced. `crates/editor-core/tests/formatting.rs` checks the new rules. The paths below are relative to `crates/editor-egui/`.

| Test | What it checked |
|---|---|
| `src/tests/edit_props.rs`: `indent_only_moves_selected_line` | Tab left an item's children in place |
| `src/tests/edit_props.rs`: `indent_is_permissive`, `shift_tab_strips_one_bq` | Shift-Tab removed a quote marker |
| `src/tests/regressions.rs`: `tab_promotes_target_child_becomes_sibling`, `tab_indent_no_list_child_uses_one_unit`, `tab_indents_parent_to_child_keeps_child_level` | Tab left an item's children in place, or aligned the item with its child |
| `src/tests/regressions.rs`: `shift_tab_mixed_container_cases` | Shift-Tab removed quote and callout markers |

## Notices

The Unlicense asks for no notice. Noura keeps this record so you can trace each file back to its source.

BSD-3-Clause requires binary distributions to reproduce the copyright notice, the license conditions and the disclaimer. `THIRD_PARTY_NOTICES.md` carries them for the `lb-rs` text model. comrak also enters the product with this fork, and `THIRD_PARTY_NOTICES.md` carries its BSD-2-Clause notice.

## Review

- Reviewer: Claude (author review). Owner review pending.
- Date: 2026-10-20. Corrected on 2026-10-20 to record the `lb-rs` text model as adapted BSD-3-Clause code. Updated on 2026-10-21 for ticket 18's changes, which copy no new source, and for ticket 19's find adaptation and removals.
