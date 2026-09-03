# Pi runtime spike

This temporary Phase 0 slice proves that Pi Agent Core can run inside Noura's WebView without access to a provider credential or direct network transport.

## Flow

1. The AI route creates a Pi `Agent` with a custom stream function.
2. The stream function creates a UUID operation ID and invokes a native Tauri Channel transport.
3. The native host sends ordered synthetic `delta`, `done`, or `aborted` frames.
4. The adapter rejects a mismatched operation ID or sequence and converts terminal native frames into Pi assistant messages.
5. Pi cancellation calls the native cancellation command. The native operation sends an `aborted` frame and removes its operation registry entry.

The probe sends neither workspace content nor provider credentials. It uses no provider request, filesystem tool, shell tool, or persistent agent state.

## Verification

Run `bun run verify:pi-runtime` to build a standalone browser bundle and reject Node-only runtime imports. With Pi `0.84.4`, the standalone bundle is 285.97 kB uncompressed and 62.85 kB gzip.

The application production build loads the probe only from the AI route. The current route chunk contribution is approximately 106.48 kB gzip; reassess this cost before making the production agent surface generally available.

## Mobile status

The iOS and Android configuration overrides use `org.noura.app`. The generated iOS project is available locally, but a full iOS compile requires Xcode and the iPhoneOS SDK. Android generation and compilation require an installed Android SDK and NDK. Those toolchains are not currently available, so mobile WebView execution remains a release gate rather than a verified claim.
