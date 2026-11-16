# handy-computer/models: speech model catalog

Noura's list of speech-to-text models is the catalog that `handy-computer/models` publishes for Handy. This record names the file taken and the rules adapted from it.

## Source

- Repository: https://github.com/handy-computer/models
- Commit: `2d77f8810ee9539a90328a4c59b420326b225d39`
- Package: `packages/models`, published as `@handy-computer/models` 0.2.3
- License: MIT, as `packages/models/package.json` declares. The repository has no license file and names no copyright holder, so Noura's notice credits "the handy-computer/models authors".

## Files taken

| Source | Destination | Changes |
|---|---|---|
| `packages/models/catalog.json` (schema 2, transcribe.cpp 0.4.0) | `crates/speech/catalog/catalog.json` | None. The file is byte for byte the same as Handy's copy at `src-tauri/src/catalog/catalog.json`. |

## Rules adapted

`crates/speech/src/catalog.rs` reads the catalog in new code and adapts these rules from `packages/models/src/recommendations.ts`:

- The wait after a dictation is the benchmark's 30 seconds divided by the model's speed on the CPU tier (`modelWaitSeconds`).
- Several languages average by their geometric mean, and a language counts as heard below `maxLanguageErrorPercent` (20).

Noura adds a filter of its own: it keeps only models whose `license` field allows commercial use (`mit`, `apache-2.0`, `cc-by-4.0`, `cc-by-sa-4.0`) and drops `cc-by-nc-*`, `cc-by-nc-sa-*` and `other`.

## Model licenses

The catalog names model files hosted on Hugging Face under the `handy-computer` organization. Noura doesn't ship any model: you download each one from Settings, and Settings shows its license. Parakeet Ultra, which onboarding recommends, is under CC BY 4.0.

## Notice

`THIRD_PARTY_NOTICES.md` carries the MIT notice for the catalog.

Reviewed by the implementing agent on 2026-11-16 for the owner's approval.
