# Third-Party Notices

noura uses third-party dependencies under their respective licenses. The lockfiles and automated license report provide the dependency inventory.

Each desktop package bundles `THIRD_PARTY_LICENSES.html`, which lists the crates the desktop app links, with their license texts. Each [GitHub release](https://github.com/lobbystack/noura/releases) attaches the same file as `third-party-licenses-<platform>.html`. [Releasing](docs/releasing.md#third-party-licenses-in-the-app) explains how the release workflow generates it.

The repository includes Apollo GraphQL's unmodified `rust-best-practices` agent skill under the MIT license. See `docs/provenance/apollo-rust-best-practices.md` for its pinned source and verification record.

The repository also includes the unmodified `shadcn-svelte` agent skill under the MIT license. See `docs/provenance/shadcn-svelte-skill.md` for its source record.

Native encryption uses age, aes-gcm, ed25519-dalek, x25519-dalek (BSD-3-Clause), hkdf (MIT/Apache-2.0), and zeroize under their permissive Cargo-declared license options. These are dependencies, not copied donor implementations. `Cargo.lock` remains the complete dependency inventory.

The v2 sync server (`sync-server`, `sync-store`, `sync-store-sqlite` and `sync-store-postgres`) uses axum (MIT), sqlx (MIT/Apache-2.0) with the SQLite it bundles (public domain) and its PostgreSQL driver, tokio and tracing (MIT), jiff (MIT/Unlicense), and hashlink (MIT/Apache-2.0) for its caches of verified chains. It reaches PostgreSQL over TLS through rustls (Apache-2.0/ISC/MIT) with ring (Apache-2.0 and ISC) and the Mozilla root certificates in `webpki-roots` (CDLA-Permissive-2.0). These are dependencies, not copied donor implementations. Atuin's server (MIT) served as a layout reference for running SQLite and PostgreSQL from one binary, and noura copied none of its code.

The sync server's account pages run passkey ceremonies with `webauthn-rs`, `webauthn-rs-core`, `webauthn-attestation-ca` and `base64urlsafedata` 0.5.5 from https://github.com/kanidm/webauthn-rs, under MPL-2.0, unmodified, and render askama templates (MIT/Apache-2.0); they send mail with lettre (MIT) or Resend's API. `account-web`, the WebAssembly module those pages load, holds `webauthn-rs-proto` and `base64urlsafedata` 0.5.5 under MPL-2.0, unmodified, with wasm-bindgen, js-sys and web-sys (MIT/Apache-2.0). The module ships with `crates/account-web/THIRD_PARTY_NOTICES.txt`, which names those crates and carries the MPL-2.0 text, and the server serves it beside the module at `/account/assets/THIRD_PARTY_NOTICES.txt`. The server's container image carries `crates/sync-server/THIRD_PARTY_NOTICES.txt` at `/usr/share/noura/THIRD_PARTY_NOTICES.txt`, which names the crates the binary links and carries the MPL-2.0 text. The server's tests sign up with passkey-rs (`passkey-client` and `passkey-authenticator`, MIT/Apache-2.0), and account-web's browser test drives Chrome through chromiumoxide (MIT/Apache-2.0). These are dependencies, not copied donor implementations.

Sync protocol v2 (`sync-protocol` and `sync-key-envelope`) uses ed25519-dalek 3 and x25519-dalek 3 (BSD-3-Clause), hpke with x-wing and ml-kem, chacha20poly1305, aead-stream and getrandom (MIT/Apache-2.0), and blake3 (CC0-1.0/Apache-2.0). These are dependencies, not copied donor implementations. Its tests read one draft-ietf-hpke-pq vector taken from the `hpke` package and the Wycheproof sets through the `wycheproof` crate (Apache-2.0); see `docs/provenance/sync-v2-test-vectors.md`.

Notable runtime dependencies include the Apache-2.0 licensed official Rust MCP SDK (`rmcp`), dual MIT/Apache-2.0 `genai`, MIT/Apache-2.0 `keyring`, and MIT `trash`. Desktop release packaging includes the generated license file described above.

The plugins that ship in each desktop package are WebAssembly components that link `wit-bindgen` (Apache-2.0 WITH LLVM-exception, Apache-2.0 or MIT), as the license file says. Their other crates also appear in the desktop binary.

Plugin calendar grids name weekdays and months and write times in your locale with ICU4X's `icu_datetime`, `icu_calendar`, `icu_time` and `icu_locale_core` (Unicode-3.0), which compile in CLDR data under the same license. `sys-locale` (MIT/Apache-2.0) reads the locale from the operating system. These are dependencies, not copied donor implementations, and the generated license file carries their license texts.

When the AI reads a web page, `local-core` decodes it with `encoding_rs` ((Apache-2.0 OR MIT) AND BSD-3-Clause), parses its HTML with `html5ever` and `markup5ever` (MIT/Apache-2.0), and turns it into text with `html2text` (MIT). These are dependencies, not copied donor implementations, and the generated license file carries their license texts.

## atomicwrites 0.4.4 (Windows device settings)

Windows settings persistence uses the MIT-licensed `atomicwrites` dependency from https://github.com/untitaker/rust-atomicwrites, pinned in Cargo.lock. No implementation source was copied. The distributed crate's license follows.

Copyright (c) 2015 Markus Unterwaditzer

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## yrs 0.26.0 (native text collaboration)

Dependency source and verification: [Yrs provenance](docs/provenance/yrs.md). `crates/local-core` uses the crates.io package unchanged. The collaboration sandbox's guest module builds from a patched copy of the package in `crates/collab-sandbox-guest/vendor/yrs`; the provenance record lists each change.

The MIT License (MIT)

Copyright (c) 2020

- Bartosz Sypytkowski <b.sypytkowski@gmail.com>
- Kevin Jahns <kevin.jahns@pm.me>.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

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

The crate's `fonts` folder holds each font's full license text. Noura uses the crate as a dependency and copies no font file from it into this repository. The interface draws in Public Sans, below, with egui's fonts behind it. On macOS and Windows the app also reads the system's own font, San Francisco or Segoe UI, at launch, for symbols such as ⌘ that Public Sans lacks, and Noura ships neither.

## Public Sans 5.3.0 (interface font)

The desktop app draws its interface and notes in Public Sans, the legacy app's font, from `@fontsource-variable/public-sans` 5.3.0. `crates/app-ui/assets/fonts` holds its Latin, Latin Extended and Vietnamese subsets, converted from WOFF2 to TTF, and the license. See [`docs/provenance/public-sans.md`](docs/provenance/public-sans.md). Every desktop package carries this license in `THIRD_PARTY_LICENSES.html`.

```text
Copyright 2015 The Public Sans Project Authors (https://github.com/uswds/public-sans) PublicSans-Italic[wght].ttf: Copyright 2015 The Public Sans Project Authors (https://github.com/uswds/public-sans)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

## Phosphor icons

The desktop app draws its interface icons from Phosphor's path data, copied from `phosphor-svelte` 3.1.0 into `crates/app-ui/src/icons/phosphor.rs`. See [`docs/provenance/phosphor-icons.md`](docs/provenance/phosphor-icons.md).

```text
MIT License

Copyright (c) 2020 Phosphor Icons

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

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

The desktop app decodes AVIF images in notes with these crates. Noura uses them unmodified, pinned in Cargo.lock, and copies none of their source. The `THIRD_PARTY_LICENSES.html` file in each desktop package carries their full license texts.

- `avif-decode` 3.0.0, under the BSD 3-Clause License: Copyright 2021 Kornel Lesiński
- `rav1d` 1.1.0, a Rust port of the dav1d AV1 decoder, under the BSD 2-Clause License: Copyright © 2018-2019, VideoLAN and dav1d authors, and Copyright © 2023-2024, VideoLAN, dav1d authors, and Internet Security Research Group
- `yuv` 0.8.19, under the BSD 3-Clause option of its `BSD-3-Clause OR Apache-2.0` license: Copyright (c) Radzivon Bartoshyk

`avif-parse`, which reads the AVIF file around the AV1 data, uses the Mozilla Public License 2.0 and appears in the MPL-2.0 list below.

## hayro 0.8.0 (PDF rendering)

The desktop app opens, draws, searches and unlocks PDFs with Laurenz Stampfl's `hayro`, `hayro-interpret` and `hayro-syntax` 0.8.0, and the crates they bring, under the MIT option of their `Apache-2.0 OR MIT` licenses. Noura uses them unmodified, pinned in Cargo.lock, and copies none of their source. The `THIRD_PARTY_LICENSES.html` file in each desktop package carries their full license texts.

`hayro-interpret` builds in two sets of files whose licenses differ from the crate's own:

- PDFium's Foxit fonts, which stand in for the standard PDF fonts a PDF doesn't embed, under the BSD 3-Clause License. Original code copyright 2014 Foxit Software Inc.
- An ICC profile for CMYK colors from Compact-ICC-Profiles, under CC0-1.0, which asks for no notice.

`hayro-cmap` builds in Adobe's CMap resources under the BSD 3-Clause License. Both notices follow.

Copyright 1990-2023 Adobe. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.

Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

Neither the name of Adobe nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Copyright 2014 PDFium Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

- Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
- Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
- Neither the name of Google Inc. nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## Speech (Handy, anarlog and handy-computer/models)

`crates/speech` records audio, runs speech-to-text models and pastes into other apps. It uses transcribe-cpp (MIT), which builds transcribe.cpp and ggml (MIT), handy-recorder (MIT), hf-hub (Apache-2.0), rubato (MIT/Apache-2.0), hound (Apache-2.0), enigo (MIT), arboard (MIT/Apache-2.0), rtrb (MIT/Apache-2.0), cidre (MIT) on macOS, wasapi (MIT) and windows (MIT/Apache-2.0) on Windows, and libpulse-binding (MIT/Apache-2.0) on Linux. The desktop app adds global-hotkey and tray-icon (MIT/Apache-2.0), objc2-user-notifications (Zlib/Apache-2.0/MIT) on macOS, and ashpd (MIT) and zbus (MIT) on Linux. The Dictation plugin matches custom words with strsim (MIT) and natural (MIT), and names languages with isolang (Apache-2.0). These are dependencies, and the generated license file carries their license texts.

Three donors' code came along, each recorded in `docs/provenance/`:

- [Handy](docs/provenance/handy.md): the macOS and Windows pastes that wait for the target app to read the clipboard, the model grades, and the Dictation plugin's shortcut modes, custom words and menu.
- [anarlog](docs/provenance/anarlog.md): system audio capture on macOS and Windows, the check on Windows and Linux for another app using the microphone, and the Transcription plugin's Summary prompts and length policy.
- [handy-computer/models](docs/provenance/handy-models.md): the model catalog, `crates/speech/catalog/catalog.json`.

Noura ships no speech model. You download each one from Hugging Face in Settings, under the license Settings shows; Parakeet Ultra uses CC BY 4.0.

### Handy

MIT License

Copyright (c) 2025 CJ Pais

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

### anarlog

MIT License

Copyright (c) 2023-present Fastrepl, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

### handy-computer/models

MIT License

Copyright (c) the handy-computer/models authors (https://github.com/handy-computer/models)

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

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
