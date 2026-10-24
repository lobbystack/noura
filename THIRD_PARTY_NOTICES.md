# Third-Party Notices

The unified hosted app includes `@tauri-apps/api` under its MIT option. Generated release notices include its copyright and license text, which also appears in [`apps/server/licenses/tauri-api-2.11.1.txt`](apps/server/licenses/tauri-api-2.11.1.txt). See [`docs/provenance/tauri-api-license.md`](docs/provenance/tauri-api-license.md).

noura uses third-party dependencies under their respective licenses. The lockfiles and automated license report provide the dependency inventory.

Each desktop release bundles `THIRD_PARTY_LICENSES.txt`, which lists the npm packages in the desktop frontend bundle and the crates the desktop app links, with their license texts. The same file is attached to each [GitHub release](https://github.com/lobbystack/noura/releases) as `third-party-licenses-<platform>.txt`. [Releasing](docs/releasing.md#third-party-licenses-in-the-app) explains how the release workflow generates it.

The repository includes Apollo GraphQL's unmodified `rust-best-practices` agent skill under the MIT license. See `docs/provenance/apollo-rust-best-practices.md` for its pinned source and verification record.

The repository also includes the unmodified `shadcn-svelte` agent skill under the MIT license. See `docs/provenance/shadcn-svelte-skill.md` for its source record.

The noura server build generates `apps/server/dist/THIRD_PARTY_NOTICES.md` from the packages present in the Bun server and browser bundles and includes it in the Docker image. The release build also inventories extracted CSS and fonts. Its additional dependencies include Hono and Better Auth (MIT), Nodemailer (MIT-0), and Postgres.js (Unlicense). The server's bundle-specific check accepts MIT-0 and Unlicense as permissive licenses. noura has not copied donor source into the synchronization service.

Resumable uploads use `@tus/server`, `@tus/file-store`, and `@tus/utils` (MIT). noura supplies the missing published utilities notice from its exact upstream commit; see `docs/provenance/tus-utils-license.md`.

The browser viewer uses Marked (MIT), DOMPurify (the Apache-2.0 option of its dual license), and Public Sans (OFL-1.1). Native and portable encryption use age, aes-gcm, ed25519-dalek, x25519-dalek (BSD-3-Clause), hkdf (MIT/Apache-2.0), and zeroize under their permissive Cargo-declared license options. These are dependencies, not copied donor implementations. The pinned Cargo and Bun lockfiles remain the complete dependency inventories.

The v2 sync server (`sync-server`, `sync-store` and `sync-store-sqlite`) uses axum (MIT), sqlx (MIT/Apache-2.0) with the SQLite it bundles (public domain), tokio and tracing (MIT), and jiff (MIT/Unlicense). These are dependencies, not copied donor implementations. Atuin's server (MIT) served as a layout reference, and noura copied none of its code.

Sync protocol v2 (`sync-protocol` and `sync-key-envelope`) uses ed25519-dalek 3 and x25519-dalek 3 (BSD-3-Clause), hpke with x-wing and ml-kem, chacha20poly1305, aead-stream and getrandom (MIT/Apache-2.0), and blake3 (CC0-1.0/Apache-2.0). These are dependencies, not copied donor implementations. Its tests read one draft-ietf-hpke-pq vector taken from the `hpke` package and the Wycheproof sets through the `wycheproof` crate (Apache-2.0); see `docs/provenance/sync-v2-test-vectors.md`.

The browser encrypted-sync foundation uses `@noble/hashes` (MIT) for BLAKE3 device fingerprints and attachment content revisions, and `@noble/ciphers` (MIT) for the ChaCha20-Poly1305 primitive inside the byte-compatible `age` attachment stream; the Rust side uses the `blake3` and `age` crates. These are dependencies, not copied donor implementations.

Notable runtime dependencies include the Apache-2.0 licensed official Rust MCP SDK (`rmcp`), dual MIT/Apache-2.0 `genai`, MIT/Apache-2.0 `keyring`, MIT `trash`, MIT `@earendil-works/pi-agent-core` and `@earendil-works/pi-ai`, MIT `Tauri`, MIT `Svelte`, MIT `Tiptap`, and CC0-1.0 `fractional-indexing`. Desktop release packaging includes the generated license file described above.

Plugin calendar grids name weekdays and months and write times in your locale with ICU4X's `icu_datetime`, `icu_calendar`, `icu_time` and `icu_locale_core` (Unicode-3.0), which compile in CLDR data under the same license. `sys-locale` (MIT/Apache-2.0) reads the locale from the operating system. These are dependencies, not copied donor implementations, and the generated license file carries their license texts.

## atomicwrites 0.4.4 (Windows device settings)

Windows settings persistence uses the MIT-licensed `atomicwrites` dependency from https://github.com/untitaker/rust-atomicwrites, pinned in Cargo.lock. No implementation source was copied. The distributed crate's license follows.

Copyright (c) 2015 Markus Unterwaditzer

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## yrs 0.26.0 (native text collaboration)

Dependency source and verification: [Yrs provenance](docs/provenance/yrs.md). No upstream implementation source was copied.

The MIT License (MIT)

Copyright (c) 2020

- Bartosz Sypytkowski <b.sypytkowski@gmail.com>
- Kevin Jahns <kevin.jahns@pm.me>.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## PDF.js and PDF rendering assets

PDF.js 6.3.289, Copyright Mozilla Foundation and contributors, is licensed under Apache-2.0. The complete license ships at `pdfjs/LICENSE`. Adobe CMaps, PDFium/Foxit fonts, and the PDFium JBIG2 decoder use BSD-3-Clause; OpenJPEG uses BSD-2-Clause; qcms uses MIT; ICC profiles use CC0-1.0. Their complete notices ship alongside their assets in `pdfjs/cmaps`, `pdfjs/standard_fonts`, `pdfjs/wasm`, and `pdfjs/iccs`.

Liberation Sans 2.1.5, digitized data Copyright 2010 Google Corporation and Copyright 2012 Red Hat, Inc., uses SIL Open Font License 1.1. Unmodified fonts and the complete license ship in `pdfjs/standard_fonts`; the source license is also retained in `apps/app/vendor/pdf-fonts/LICENSE`. The older GPL Liberation files from the PDF.js package are excluded from the application bundle. See `docs/provenance/pdfjs.md` for exact sources and revisions.

## security-framework 3.7.0 (macOS Keychain interaction)

Native credential access uses the MIT option of the MIT/Apache-2.0 licensed `security-framework` dependency from https://github.com/kornelski/rust-security-framework. The version and checksum are pinned in Cargo.lock. No implementation source was copied.

The MIT License (MIT)

Copyright (c) 2015 Steven Fackler

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## egui default fonts

The desktop app draws text with the fonts egui bundles in `epaint_default_fonts` 0.37.0:

- **Ubuntu Light**: Copyright 2011 Canonical Ltd., under the Ubuntu Font Licence 1.0
- **Noto Emoji**: Copyright 2013 Google Inc., under the SIL Open Font License 1.1
- **Hack**: Copyright 2018 Source Foundry Authors, under the MIT License, with Bitstream Vera glyphs Copyright 2003 Bitstream, Inc., under the Bitstream Vera License
- **emoji-icon-font** and its `egui-icons` subset: Copyright 2014 John Slegers, under the MIT License

The crate's `fonts` folder holds each font's full license text. Noura uses the crate as a dependency and copies no font file into this repository.

## comrak 0.50.0 (Markdown parser)

The editor parses Markdown with `comrak`, a dependency pinned in Cargo.lock. Noura copies no comrak source. Its distributed `COPYING` file carries the notices below for the code the editor ships: comrak under the BSD 2-Clause License, the cmark-gfm code it derives from under the same license, and the houdini, buffer and utf8proc code it adapts under the MIT License. The CommonMark spec and test files that `COPYING` also covers stay out of the build.

Copyright (c) 2017–2025, Comrak contributors

All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

- Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
- Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

cmark-gfm, derived from https://github.com/github/cmark: Copyright (c) 2014, John MacFarlane. All rights reserved. Licensed under the BSD 2-Clause License above.

houdini (`houdini.h`, `houdini_href_e.c`, `houdini_html_e.c`, `houdini_html_u.c`), from https://github.com/vmg/houdini: Copyright (C) 2012 Vicent Martí. `buffer.h`, `buffer.c` and `chunk.h`: Copyright 2012 GitHub, Inc. `utf8.c`, from utf8proc (http://www.public-software-group.org/utf8proc): Copyright 2009 Public Software Group e. V., Berlin, Germany. Each is licensed as follows:

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## arborium 2.18.2 and its grammars (code highlighting)

The editor highlights fenced code in notes with `arborium` 2.18.2, used unmodified and pinned in Cargo.lock, under the MIT option of its `MIT OR Apache-2.0` license: Copyright (c) 2024 Amos Wenger. Noura copies none of its source. arborium links the tree-sitter runtime (`arborium-tree-sitter`, Copyright (c) 2018-2024 Max Brunsfeld) and parsers generated from these grammars, each under the MIT License:

- Bash (tree-sitter/tree-sitter-bash): Copyright (c) 2017 Max Brunsfeld
- C, C++, Go, HTML, JavaScript and JSON (tree-sitter/tree-sitter-c, -cpp, -go, -html, -javascript, -json): Copyright (c) 2014 Max Brunsfeld
- C# (tree-sitter/tree-sitter-c-sharp): Copyright (c) 2014-2023 Max Brunsfeld, Damien Guard, Amaan Qureshi, and contributors
- CSS and JSDoc (tree-sitter/tree-sitter-css, -jsdoc): Copyright (c) 2018 Max Brunsfeld
- Diff (the-mikedavis/tree-sitter-diff): Copyright (c) 2021 Michael Davis
- Java (tree-sitter/tree-sitter-java): Copyright (c) 2017 Ayman Nadeem
- Kotlin (fwcd/tree-sitter-kotlin): Copyright (c) 2019 fwcd
- Lua (tree-sitter-grammars/tree-sitter-lua): Copyright (c) 2021 Munif Tanjim
- Markdown (tree-sitter-grammars/tree-sitter-markdown): Copyright (c) 2021 Matthias Deiml
- PHP (tree-sitter/tree-sitter-php): Copyright (c) 2017 Josh Vera, GitHub, and Copyright (c) 2019 Max Brunsfeld, Amaan Qureshi, Christian Frøystad, Caleb White
- Python (tree-sitter/tree-sitter-python): Copyright (c) 2016 Max Brunsfeld
- Ruby (tree-sitter/tree-sitter-ruby): Copyright (c) 2016 Rob Rix
- Rust (grammar-orchard/tree-sitter-rust-orchard on Codeberg): Copyright (c) 2017 Maxim Sokolov
- SQL (DerekStride/tree-sitter-sql): Copyright (c) 2021 Derek Stride
- Swift (alex-pinkus/tree-sitter-swift): Copyright (c) 2021 alex-pinkus
- TOML (tree-sitter-grammars/tree-sitter-toml): Copyright (c) Ika
- TypeScript and TSX (tree-sitter/tree-sitter-typescript): Copyright (c) 2017 Max Brunsfeld
- XML (tree-sitter-grammars/tree-sitter-xml): Copyright (c) 2023 ObserverOfTime
- YAML (tree-sitter-grammars/tree-sitter-yaml): Copyright (c) 2024 tree-sitter-grammars contributors, and Copyright (c) 2019-2021 Ika

Each grammar's crate README records the upstream commit it was generated from. The MIT License for arborium, the runtime and each grammar:

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## AVIF decoding

The desktop app decodes AVIF images in notes with these crates. Noura uses them unmodified, pinned in Cargo.lock, and copies none of their source. The `THIRD_PARTY_LICENSES.txt` file in each desktop package carries their full license texts.

- `avif-decode` 3.0.0, under the BSD 3-Clause License: Copyright 2021 Kornel Lesiński
- `rav1d` 1.1.0, a Rust port of the dav1d AV1 decoder, under the BSD 2-Clause License: Copyright © 2018-2019, VideoLAN and dav1d authors, and Copyright © 2023-2024, VideoLAN, dav1d authors, and Internet Security Research Group
- `yuv` 0.8.19, under the BSD 3-Clause option of its `BSD-3-Clause OR Apache-2.0` license: Copyright (c) Radzivon Bartoshyk

`avif-parse`, which reads the AVIF file around the AV1 data, uses the Mozilla Public License 2.0 and appears in the MPL-2.0 list below.

## Lockbook Markdown editor

The editor in `crates/editor-core` and `crates/editor-egui` adapts Lockbook's Markdown editor, which its authors dedicated to the public domain under the Unlicense. The Unlicense asks for no notice. [The Lockbook editor provenance record](docs/provenance/lockbook-editor.md) lists the files taken and the changes made.

`editor-core`'s text buffer, offsets, grapheme index and word diff adapt the text model in Lockbook's `lb-rs` crate (`libs/lb/lb-rs/src/model/text/` at commit `1cd633a`), which its manifest licenses under the BSD 3-Clause License. `lb-rs` names no copyright holder, so this notice credits the Lockbook authors:

Copyright (c) 2023–2026, the Lockbook authors (https://github.com/lockbook/lockbook)

All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## BSL-1.0 crates (Windows clipboard)

eframe reaches the Windows clipboard through arboard, which uses `clipboard-win` 5.4.1 and `error-code` 3.4.0 by Douman (https://github.com/DoumanAsh). Noura uses both unmodified, pinned in Cargo.lock. Both use the Boost Software License 1.0:

Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization obtaining a copy of the software and accompanying documentation covered by this license (the "Software") to use, reproduce, display, distribute, execute, and transmit the Software, and to prepare derivative works of the Software, and to permit third-parties to whom the Software is furnished to do so, all subject to the following:

The copyright notices in the Software and this entire statement, including the above license grant, this restriction and the following disclaimer, must be included in all copies of the Software, in whole or in part, and all derivative works of the Software, unless such copies or derivative works are solely in the form of machine-executable object code generated by a source language processor.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## 0BSD crates (single-instance handling)

A second launch of the desktop app finds the running copy through `interprocess` 2.4.6, used under the 0BSD option of its `0BSD OR Apache-2.0` license. interprocess depends on `doctest-file` 1.1.1 and, on Windows, `recvmsg` 1.0.0, both under the BSD Zero Clause License. Noura uses all three unmodified, pinned in Cargo.lock, and copies none of their source. The BSD Zero Clause License asks for no notice.

## MPL-2.0 crates

These Rust dependencies use the Mozilla Public License 2.0. Noura uses them unmodified, and crates.io serves their source at the versions pinned in Cargo.lock:

- `webauthn-rs`, `webauthn-rs-core`, `webauthn-rs-proto`, `webauthn-attestation-ca` and `base64urlsafedata` 0.5.5, which give `sync-server` its passkey support
- `option-ext` 0.2.0, which `local-core` gets through `directories` and `dirs-sys`
- `nucleo-matcher` 0.3.1, which ranks the desktop app's command palette and quick open in `app-core`
- `avif-parse` 2.1.0, which reads AVIF images in notes for `app-ui`

## Marketing website source record

- Source: a local predecessor workspace's `apps/website` (not a public repository)
- Repository URL: unavailable; the local predecessor snapshot has no Git metadata
- Exact revision: content-addressed local snapshot (no commit was available)
- Source hashes:
  - `src/routes/+page.svelte`: `a6f6365a332b21c1df383851113bf4d78560d0652d0702e29e39cf3cb0ec12b2`
  - `src/lib/components/ShaderBackground.svelte`: `c576bbce69daafa62ed4bccf1fe535d441220192cc8eb9d4f1d1f34cd1f83e28`
- Source paths: `apps/website/`
- License: MIT, as declared by the predecessor workspace
- Destination: `apps/website/`
- Modifications: integrated into the Bun workspace; rewrote product, storage, privacy, platform, and AI claims for the current noura architecture; retained the layout, responsive styling, and WebGL shader treatment
- Notice obligation: retain the predecessor MIT notice below
- Review: source and license inspected by the migration assistant; migration requested by Raphael on 2026-09-11
- Migration date: 2026-09-11

The copied source came from a local predecessor of noura, not directly from an external donor repository. The predecessor directory does not contain `.git`, so a commit identifier cannot be recovered. The hashes above pin the exact material reviewed for this migration.

### Predecessor license notice

MIT License

Copyright (c) 2018 Johannes Millan

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
