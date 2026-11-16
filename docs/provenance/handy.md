# Handy: paste receipts and model grades

Noura's paste on macOS and the grades on Settings > Speech adapt code from Handy, the open source dictation app. This record lists each source path and what changed.

## Source

- Repository: https://github.com/cjpais/Handy
- Commit: `aca8985190c3ad7f361d8a80cb30f179513ed690`
- License: MIT, `LICENSE` at the repository root, "Copyright (c) 2025 CJ Pais". No nearer license file covers the paths below.

## Code adapted

| Source | Destination | Changes |
|---|---|---|
| `src-tauri/src/paste_tx/mod.rs`: `TxState`, `evaluate`, `QUIET_PERIOD`, `RESTORE_TIMEOUT`, `FAILED_INJECTION_TIMEOUT` and the 100 ms chord hold | `crates/speech/src/paste.rs`: `Transaction`, `Transaction::done` and the constants | Renamed, without Tauri, logging or auto-submit. The wait runs on the calling thread instead of a waiter thread. |
| `src-tauri/src/paste_tx/macos.rs`: the `HandyPasteProvider` class, the concealment types and the `declareTypes:owner:` publish | `crates/speech/src/paste.rs`, `platform` module for macOS (`NouraPasteProvider`) | The class is renamed. The saved clipboard is text only, read and restored through `arboard` instead of Tauri's clipboard plugin. No image restore, auto-submit or "keep the transcript" mode. |
| `src-tauri/src/catalog/mod.rs`: `CatalogModel::is_offered` (a transcription model whose default file sits at the repository's top level) | `crates/speech/src/catalog.rs`, `Catalog::parse` | Rewritten in Noura's types, with Noura's license filter added. |
| `src/lib/models/catalog.ts`: `GRADE_CEILINGS` (A 5%, B 10%, C 20%, D 30%) | `crates/speech/src/catalog.rs`: `GRADE_CEILINGS` and `Grade` | Letters only, without plus and minus. |
| `src/lib/models/grading.ts`: the geometric mean over your languages | `crates/speech/src/catalog.rs`: `Catalog::recommended` | Rewritten in Rust. |

## Left behind

Handy's Windows paste receipts (`src-tauri/src/paste_tx/windows.rs`, delayed rendering with `WM_RENDERFORMAT`) didn't come along. Noura's Windows paste restores the clipboard after a fixed 500 ms.

Settings > Speech follows the layout of Handy's Models page (`src/components/settings/models/`) in new egui code; no TypeScript or CSS came along.

## Notice

`THIRD_PARTY_NOTICES.md` carries Handy's MIT notice.

Reviewed by the implementing agent on 2026-11-16 for the owner's approval.
