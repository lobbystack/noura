# Phosphor icons

Noura's interface icons are Phosphor's. The legacy app drew the same icons through `phosphor-svelte`, and the Rust app keeps them so both look alike.

## Source

- Package: `phosphor-svelte` 3.1.0 from npm, the version the legacy app's `bun.lock` pinned
- Integrity: `sha512-nldtxx+XCgNREvrb7O5xgDsefytXpSkPTx8Rnu3f2qQCUZLDV1rLxYSd2Jcwckuo9lZB1qKMqGR17P4UDC0PrA==`
- Source path: `lib/<Name>.svelte`, the `<path d="...">` data of the `regular` and `fill` weights
- Upstream project: https://github.com/phosphor-icons/core
- License: MIT, Copyright (c) 2020 Phosphor Icons, from the package's `LICENSE`

## Destination

`crates/app-ui/src/icons/phosphor.rs` holds the path data of these icons, in both weights: Tray, Files, Checks, Calendar, FolderOpen, MagnifyingGlass, GearSix, Folder, FileText, FilePdf, CaretRight, CaretDown, NotePencil, FolderSimplePlus, ArrowsInLineVertical, Plus, X, CaretLeft, CheckCircle, CalendarBlank, TextAa, Keyboard, UserCircle, FolderSimple, PuzzlePiece, PlugsConnected, ArrowsClockwise, Users, Info, Sun, Moon, Desktop, ArrowUpRight, Copy, Check, Warning, Circle, Kanban, File, PaperPlaneTilt, ChatsCircle, DotsThree, Trash, FolderPlus, ArrowSquareOut, UploadSimple, Stop, ArrowCounterClockwise, Spinner, ArrowLeft, XCircle, Key, DownloadSimple, Translate and Broadcast.

## Modifications

A script copied each path string as it is, into one Rust constant per icon and weight. `crates/app-ui/src/icons/mod.rs` wraps each path in an SVG of Phosphor's 256-unit grid and renders it with resvg. Noura changed no path.

## Notices

`THIRD_PARTY_NOTICES.md` carries the MIT notice, and the desktop app's generated license file lists it.

## Review

Reviewed by Claude for the repository owner on 2026-11-03.
