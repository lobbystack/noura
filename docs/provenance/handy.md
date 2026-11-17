# Handy: paste receipts, model grades and dictation

Noura's paste on macOS and Windows, the grades on Settings > Speech and the Dictation plugin adapt code from Handy, the open source dictation app. This record lists each source path and what changed.

## Source

- Repository: https://github.com/cjpais/Handy
- Commit: `aca8985190c3ad7f361d8a80cb30f179513ed690`
- License: MIT, `LICENSE` at the repository root, "Copyright (c) 2025 CJ Pais". No nearer license file covers the paths below.

## Code adapted

| Source | Destination | Changes |
|---|---|---|
| `src-tauri/src/paste_tx/mod.rs`: `TxState`, `evaluate`, `QUIET_PERIOD`, `RESTORE_TIMEOUT`, `FAILED_INJECTION_TIMEOUT` and the 100 ms chord hold | `crates/speech/src/paste.rs`: `Transaction`, `Transaction::done` and the constants | Renamed, without Tauri, logging or auto-submit. The wait runs on the calling thread instead of a waiter thread. |
| `src-tauri/src/paste_tx/macos.rs`: the `HandyPasteProvider` class, the concealment types and the `declareTypes:owner:` publish | `crates/speech/src/paste.rs`, `platform` module for macOS (`NouraPasteProvider`) | The class is renamed. The saved clipboard is text only, read and restored through `arboard` instead of Tauri's clipboard plugin. No image restore, auto-submit or "keep the transcript" mode. |
| `src-tauri/src/paste_tx/windows.rs`: the message-only window and its procedure (`WM_RENDERFORMAT` as the read receipt, `WM_RENDERALLFORMATS`, `WM_DESTROYCLIPBOARD`, the 25 ms timer), `render_text`, `restore_snapshot`, `snapshot_clipboard` with its skipped formats and bitmap copy, `publish` and `publish_formats` with the three clipboard-history markers and the delayed-render `SetClipboardData` check | `crates/speech/src/paste.rs`, `platform` module for Windows (`NouraPasteWindow`) | Uses Noura's `Transaction` for the receipts and the timeouts. `paste` holds a lock so one paste owns the clipboard at a time, which replaces Handy's `PENDING` and `flush_pending`, and it waits for the window's thread, so it returns once the clipboard is back. No auto-submit or "keep the transcript" mode, no Tauri handle or logging. |
| `src-tauri/src/catalog/mod.rs`: `CatalogModel::is_offered` (a transcription model whose default file sits at the repository's top level) | `crates/speech/src/catalog.rs`, `Catalog::parse` | Rewritten in Noura's types, with Noura's license filter added. |
| `src/lib/models/catalog.ts`: `GRADE_CEILINGS` (A 5%, B 10%, C 20%, D 30%) | `crates/speech/src/catalog.rs`: `GRADE_CEILINGS` and `Grade` | Letters only, without plus and minus. |
| `src/lib/models/grading.ts`: the geometric mean over your languages | `crates/speech/src/catalog.rs`: `Catalog::recommended` | Rewritten in Rust. |
| `src-tauri/src/transcription_coordinator.rs`: `CoordinatorState` with `on_input`, `classify_busy_input`, `finish_hold`, `finish_pending_hold`, `on_processing_finished`, `on_start_result`, `on_cancel`, `DEBOUNCE` and their tests | `crates/plugins/dictation/src/activation.rs`: `Activation` and its tests | One shortcut instead of several bindings, so no binding IDs. Times are milliseconds from the plugin's clock instead of `Instant`. No release grace (`RELEASE_GRACE`, `PendingRelease`): a plugin has no timer, so a release in a hold mode resolves at once. No external-trigger flag. No coordinator thread or Tauri events. |
| `src-tauri/src/settings.rs`: `ShortcutActivation` (toggle, push-to-talk, hold or toggle, the default), `default_hold_threshold_ms` (300) and `default_word_correction_threshold` (0.18) | `crates/plugins/dictation/src/activation.rs`: `Mode` and `HOLD_THRESHOLD_MS`; `crates/plugins/dictation/src/words.rs`: `THRESHOLD` | Values written as `hold-or-toggle`, `push-to-talk` and `toggle` in `Settings/Dictation.md`. The threshold isn't a setting. |
| `src-tauri/src/audio_toolkit/text.rs`: `apply_custom_words`, `build_ngram`, `build_match_key`, `CustomWordMatchKey`, `build_custom_word_match_keys`, `is_supported_fuzzy_key`, `supports_soundex`, `find_best_match`, `preserve_case_pattern`, `extract_punctuation`, `collapse_stutters`, `normalize_transcription_output` and the custom-word tests | `crates/plugins/dictation/src/words.rs` | Comments shortened, clippy's suggestions taken. `normalize_transcription_output` becomes part of `finish` without the regex, since `collapse_stutters` already joins words with one space. No filler-word removal or Chinese script conversion. |
| `src-tauri/src/managers/transcription.rs`: the check that a transcript holds a letter or digit before it pastes | `crates/plugins/dictation/src/words.rs`: `finish` | Rewritten. |
| `src/components/settings/CustomWords.tsx`: the custom word's cleanup (no `<>"'`, one space, trimmed, at most 50 characters) and the duplicate check | `crates/plugins/dictation/src/words.rs`: `clean_word`; `crates/plugins/dictation/src/settings.rs`: `Change::apply` | Rewritten in Rust. |
| `src-tauri/src/languages.rs`: `OVERRIDES` and `language_name` | `crates/plugins/dictation/src/languages.rs`: `name` | Adds `no` to the overrides and drops ISO's qualifiers after `;` or `(`. Handy's `language_identity` became a split at `-` or `_`. |
| `src-tauri/src/tray.rs`: the menu's layout (Copy Last Transcript, a model submenu named after the active model with a check on it, a Language submenu shown when a model hears two or more languages, Other Languages, and Cancel while busy) and `src/i18n/locales/en/translation.json`'s tray and activation labels | `crates/plugins/dictation/src/menu.rs`; `crates/plugins/dictation/src/dictation.rs` | Rewritten as entries for Noura's `menu-bar` interface. "Transcript" becomes "Dictation". Auto becomes Default, the language Settings > Speech holds. Without a list of the languages you speak, past 12 languages only the chosen one stays outside Other Languages. No version line, unload, update or settings items: Noura adds Settings… and Quit. |

## Left behind

Linux keeps a timed restore after 500 ms: neither X11 nor Wayland tells the clipboard's owner when an app reads it in a way Noura can tie to one paste.

Handy's recording overlay, history, filler-word removal and post-processing didn't come along: the plugin has no window of its own, and the spec leaves them out.

Settings > Speech follows the layout of Handy's Models page (`src/components/settings/models/`) in new egui code; no TypeScript or CSS came along.

## Notice

`THIRD_PARTY_NOTICES.md` carries Handy's MIT notice.

Reviewed by the implementing agents on 2026-11-16 for the owner's approval. The Windows paste row was added on 2026-11-16 from the same commit.
