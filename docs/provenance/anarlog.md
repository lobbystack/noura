# anarlog: system audio capture and meeting summaries

Noura records what other apps play with code adapted from anarlog's audio crate, and the Transcription plugin writes its Summary with anarlog's prompts and length policy. This record lists each source path and what changed.

## Source

- Repository: https://github.com/fastrepl/anarlog
- Commit: `ad5cc8b4542c2d352318b3a91560969442cba450`
- License: MIT, `LICENSE` at the repository root, "Copyright (c) 2023-present Fastrepl, Inc.". `LICENSING.md` puts every path outside `enterprise/` under that license. Noura took nothing from `enterprise/`, and `crates/audio-actual` holds no nearer license file.

## Code adapted

| Source | Destination | Changes |
|---|---|---|
| `crates/audio-actual/src/speaker/macos.rs`: the mono global process tap, the private aggregate device description and the IO proc that reads the tap's buffer | `crates/speech/src/audio/system.rs`, `macos` module | Ported from cidre 0.15 to 0.30. The IO proc pushes f32 samples into an `rtrb` ring instead of anarlog's ring and waker, and a worker thread resamples to 16 kHz with Noura's `MonoResampler`. Only f32 tap formats are read. No async stream, sample rate probe or dropped-sample counters. |
| `crates/audio-actual/src/speaker/windows.rs`: `capture_audio_loop`, `open_loopback_capture`, `open_endpoint_loopback_client` and `drain_packets` | `crates/speech/src/audio/system.rs`, `windows` module | Ported from wasapi 0.23 to 0.27, which fixes RUSTSEC-2026-0332. Opens the loopback stream at 16 kHz with WASAPI's conversion and mixes every channel to mono instead of keeping the first. Follows the default output device once a second without anarlog's confirmation count, and doesn't follow a non-default device that another app plays to. No ring buffer, waker or integer formats. |

## Transcription plugin

| Source | Destination | Changes |
|---|---|---|
| `crates/template-app/assets/enhance.system.md.jinja` | `crates/plugins/transcription/templates/enhance.system.md.jinja` | None. anarlog renders it with minijinja too. |
| `crates/template-app/assets/enhance.format.md.jinja` | `crates/plugins/transcription/templates/enhance.format.md.jinja` | None. |
| `crates/template-app/assets/enhance.user.md.jinja`, with the `session_context_non_opt`, `participants_list` and `transcripts` macros of `crates/template-app/assets/_macros.jinja` | `crates/plugins/transcription/templates/enhance.user.md.jinja` | Rewritten from askama's Rust expressions (`if let`, `match`, `.is_empty()`) into minijinja tests with the same output. The macros are inlined. The session is the Transcription's title and its times, the participants are its `attendees` by name, the transcript lines are `Me: …` and `Others: …`, and the Output Template block is gone, since a Transcription has no summary template. |
| `crates/template-app/src/enhance.rs`: `render_enhance_system`, `normalize_format_override` and `heading_section` | `crates/plugins/transcription/src/prompt.rs` | The format override is the prompt you edit in the plugin's settings. Unknown variables fail the same way. The date comes from Noura's clock in your time zone. |
| `crates/askama-utils/src/filters.rs`: `language_name` and `extract_iso639` | `crates/plugins/transcription/src/prompt.rs` | Same rule, through the published `isolang` crate: an ISO 639-1 code, region dropped, English when unknown. |
| `crates/summary/src/lib.rs`: `summary_length_policy`, `summary_length_policy_for_texts`, `count_normalized_characters` and the JavaScript whitespace helpers they use | `crates/plugins/transcription/src/length.rs` | No tags, titles or `specta` types. |
| `apps/desktop/src/services/enhancer/summary-length.ts`: `formatSummaryLengthGuidance`, and `withLengthGuidance` in `apps/desktop/src/store/zustand/ai-task/task-configs/enhance-workflow.ts` | `crates/plugins/transcription/src/length.rs` | Ported from TypeScript to Rust, for a summary without template sections. |
| `apps/desktop/src/settings/schema.ts`: the `ai_language` setting, an ISO 639-1 code that defaults to `en`, and `custom_summary_instructions` | `crates/plugins/transcription/src/settings.rs` | Stored in the plugin's settings file in the workspace. With no language set, the Summary uses the language Settings, Speech holds, then English. |

## Not taken

anarlog's mic capture (`crates/audio-actual/src/mic.rs`) and its recovery code didn't come along: the microphone goes through the published `handy-recorder` crate, and Noura's own loop in `crates/speech/src/audio/mic.rs` reopens the default device. On Linux, Noura records the PulseAudio monitor through `handy-recorder` instead of anarlog's PipeWire and PulseAudio code in `speaker/linux.rs`.

## Notice

`THIRD_PARTY_NOTICES.md` carries anarlog's MIT notice.

Reviewed by the implementing agent on 2026-11-16 for the owner's approval.
