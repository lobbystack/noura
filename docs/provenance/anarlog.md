# anarlog: system audio capture

Noura records what other apps play with code adapted from anarlog's audio crate. This record lists each source path and what changed.

## Source

- Repository: https://github.com/fastrepl/anarlog
- Commit: `ad5cc8b4542c2d352318b3a91560969442cba450`
- License: MIT, `LICENSE` at the repository root, "Copyright (c) 2023-present Fastrepl, Inc.". `LICENSING.md` puts every path outside `enterprise/` under that license. Noura took nothing from `enterprise/`, and `crates/audio-actual` holds no nearer license file.

## Code adapted

| Source | Destination | Changes |
|---|---|---|
| `crates/audio-actual/src/speaker/macos.rs`: the mono global process tap, the private aggregate device description and the IO proc that reads the tap's buffer | `crates/speech/src/audio/system.rs`, `macos` module | Ported from cidre 0.15 to 0.30. The IO proc pushes f32 samples into an `rtrb` ring instead of anarlog's ring and waker, and a worker thread resamples to 16 kHz with Noura's `MonoResampler`. Only f32 tap formats are read. No async stream, sample rate probe or dropped-sample counters. |
| `crates/audio-actual/src/speaker/windows.rs`: `capture_audio_loop`, `open_loopback_capture`, `open_endpoint_loopback_client` and `drain_packets` | `crates/speech/src/audio/system.rs`, `windows` module | Ported from wasapi 0.23 to 0.27, which fixes RUSTSEC-2026-0332. Opens the loopback stream at 16 kHz with WASAPI's conversion and mixes every channel to mono instead of keeping the first. Follows the default output device once a second without anarlog's confirmation count, and doesn't follow a non-default device that another app plays to. No ring buffer, waker or integer formats. |

## Not taken

anarlog's mic capture (`crates/audio-actual/src/mic.rs`) and its recovery code didn't come along: the microphone goes through the published `handy-recorder` crate, and Noura's own loop in `crates/speech/src/audio/mic.rs` reopens the default device. On Linux, Noura records the PulseAudio monitor through `handy-recorder` instead of anarlog's PipeWire and PulseAudio code in `speaker/linux.rs`.

## Notice

`THIRD_PARTY_NOTICES.md` carries anarlog's MIT notice.

Reviewed by the implementing agent on 2026-11-16 for the owner's approval.
