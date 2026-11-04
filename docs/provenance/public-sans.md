# Public Sans

Noura's interface font is Public Sans, the font the legacy app bundled. The Rust app bundles the same files so both read alike.

## Source

- Package: `@fontsource-variable/public-sans` 5.3.0 from npm, the version `bun.lock` pins for `apps/app`
- Integrity: `sha512-AVfkmAt50BMXWpOO21FAntiJFKGX6xTc2dSL8dxtDteONe9IuRXJWGbs0EbG955vAMCq23ENeuopuW87cGWDSQ==`
- Source path: `files/public-sans-{latin,latin-ext,vietnamese}-wght-{normal,italic}.woff2`, and `LICENSE`
- Upstream project: https://github.com/uswds/public-sans
- License: SIL Open Font License 1.1, Copyright 2015 The Public Sans Project Authors, from the package's `LICENSE`

## Destination

`crates/app-ui/assets/fonts` holds six fonts, `PublicSans-latin.ttf`, `PublicSans-latin-ext.ttf` and `PublicSans-vietnamese.ttf` with an `-Italic` file for each, and the license as `OFL.txt`. `crates/app-ui/src/fonts.rs` embeds them. egui draws the interface with the upright files, and the note editor shapes text with all six.

## Modifications

`scripts/woff2-to-ttf.mjs` turned each WOFF2 file into a TTF: it undoes the WOFF2 compression and rebuilds the `glyf` and `loca` tables WOFF2 stores transformed, writing a long `loca`. Run `node scripts/woff2-to-ttf.mjs <file>.woff2 <file>.ttf` from the repository root to repeat it. Noura changed no outline, metric or name. The fonts keep the name Public Sans, which the license allows because Noura only converted their format.

The SHA-256 of each source file and its TTF:

| Source | TTF |
| --- | --- |
| `public-sans-latin-wght-normal.woff2` `5ed4d31c988e73b258894244f209069ebe77dc7e564861954b21198b6de90d68` | `PublicSans-latin.ttf` `ebd20736fe1273a4d67854189d016cddb348a805029ee8b193097f3fccfa37e2` |
| `public-sans-latin-wght-italic.woff2` `16dc93252adb78785ae56a6465494f73b604b39817760ea92bd4046521bb5a35` | `PublicSans-latin-Italic.ttf` `711f7b4e92bdb7acb20be3988e6d5ef2f27b16ef1d480db11b833fe6b9d8c312` |
| `public-sans-latin-ext-wght-normal.woff2` `3a00a32f0242b723dcea79935747d6d27dd93675d03ef23f470dfe274e79586a` | `PublicSans-latin-ext.ttf` `331ebc578c9c0590318e537b78f2158fd78281f61339db1540777ac57dba41b9` |
| `public-sans-latin-ext-wght-italic.woff2` `a071e35bbfc9c62756e0beb2475bff387d456dbfb2ec7e0d4e90cadeb543b54a` | `PublicSans-latin-ext-Italic.ttf` `d6341aa4db157c549bada85415597e55a76e32327b5958faadfb72bf5935f4b1` |
| `public-sans-vietnamese-wght-normal.woff2` `94593e69fad0885b2b0b74cd18d036c7e9cd6051b2256ab281a20f5bc9ccb46f` | `PublicSans-vietnamese.ttf` `0ed44ee9d4aad91cc45a644a1ee8ea24206c3de8eae436415670597ef8ea0ea3` |
| `public-sans-vietnamese-wght-italic.woff2` `fadef399b5845ca0486e0eafb76b433fd0a0794d57b13c62b2f17c657a00f9ba` | `PublicSans-vietnamese-Italic.ttf` `6b5fafdcd87273b09b932a536cc60c8b4af6176b41af02049a4846de6ee15cdd` |

## Notices

`THIRD_PARTY_NOTICES.md` carries the full license. `apps/desktop/packaging/about.hbs` puts it in the `THIRD_PARTY_LICENSES.html` every desktop package ships, and the app's smoke run reports the list missing when it lacks the license.

## Review

The repository owner approved the SIL Open Font License 1.1 for this font. Claude prepared the files for the owner on 2026-11-04.
