# Third-Party Notices

The unified hosted app includes `@tauri-apps/api` under its MIT option. Its
copyright and license text are in
[`apps/server/licenses/tauri-api-2.11.1.txt`](apps/server/licenses/tauri-api-2.11.1.txt)
and are included in generated release notices. See
[`docs/provenance/tauri-api-license.md`](docs/provenance/tauri-api-license.md).

Noura uses third-party dependencies under their respective licenses. The lockfiles and automated license report provide the dependency inventory.

The repository includes Apollo GraphQL's unmodified `rust-best-practices` agent skill under the MIT license. See `docs/provenance/apollo-rust-best-practices.md` for its pinned source and verification record.

The repository also includes the unmodified `shadcn-svelte` agent skill under the MIT license. See `docs/provenance/shadcn-svelte-skill.md` for its source record.

The Noura server build generates `apps/server/dist/THIRD_PARTY_NOTICES.md` from
the packages actually present in the Bun server and browser bundles and includes
it in the Docker image. The release build also inventories extracted CSS and
fonts. Its additional dependencies include Hono and Better Auth (MIT),
Nodemailer (MIT-0), and Postgres.js (Unlicense). MIT-0 and Unlicense are
permissive licenses accepted by the server's bundle-specific check. No donor
source has been copied into the synchronization service.

Resumable uploads use `@tus/server`, `@tus/file-store`, and `@tus/utils`
(MIT). The missing published utilities notice is supplied from its exact
upstream commit; see `docs/provenance/tus-utils-license.md`.

The browser viewer uses Marked (MIT), DOMPurify (the Apache-2.0 option of its
dual license), and Public Sans (OFL-1.1). Native encryption uses age, aes-gcm,
ed25519-dalek, and zeroize under their permissive Cargo-declared license options.
These are dependencies, not copied donor implementations. The pinned Cargo and
Bun lockfiles remain the complete dependency inventories.

Notable runtime dependencies include the Apache-2.0 licensed official Rust MCP SDK (`rmcp`), dual MIT/Apache-2.0 `genai`, MIT/Apache-2.0 `keyring`, MIT `@earendil-works/pi-agent-core` and `@earendil-works/pi-ai`, MIT `Tauri`, MIT `Svelte`, MIT `Tiptap`, and CC0-1.0 `fractional-indexing`. Release packaging must include the complete lockfile-derived license report and any license texts required by the selected distribution format.

## atomicwrites 0.4.4 (Windows device settings)

Windows settings persistence uses the MIT-licensed `atomicwrites` dependency
from https://github.com/untitaker/rust-atomicwrites, pinned in Cargo.lock.
No implementation source was copied. The distributed crate's license follows.

Copyright (c) 2015 Markus Unterwaditzer

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## yrs 0.26.0 (native text collaboration)

Dependency source and verification: [Yrs provenance](docs/provenance/yrs.md).
No upstream implementation source was copied.

The MIT License (MIT)

Copyright (c) 2020

- Bartosz Sypytkowski <b.sypytkowski@gmail.com>
- Kevin Jahns <kevin.jahns@pm.me>.

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

## PDF.js and PDF rendering assets

PDF.js 6.3.289, Copyright Mozilla Foundation and contributors, is licensed under Apache-2.0. The complete license ships at `pdfjs/LICENSE`. Adobe CMaps, PDFium/Foxit fonts, and the PDFium JBIG2 decoder use BSD-3-Clause; OpenJPEG uses BSD-2-Clause; qcms uses MIT; ICC profiles use CC0-1.0. Their complete notices ship alongside their assets in `pdfjs/cmaps`, `pdfjs/standard_fonts`, `pdfjs/wasm`, and `pdfjs/iccs`.

Liberation Sans 2.1.5, digitized data Copyright 2010 Google Corporation and Copyright 2012 Red Hat, Inc., uses SIL Open Font License 1.1. Unmodified fonts and the complete license ship in `pdfjs/standard_fonts`; the source license is also retained in `apps/app/vendor/pdf-fonts/LICENSE`. The older GPL Liberation files from the PDF.js package are excluded from the application bundle. See `docs/provenance/pdfjs.md` for exact sources and revisions.

## security-framework 3.7.0 (macOS Keychain interaction)

Native credential access uses the MIT option of the MIT/Apache-2.0 licensed
`security-framework` dependency from https://github.com/kornelski/rust-security-framework.
The version and checksum are pinned in Cargo.lock. No implementation source was copied.

The MIT License (MIT)

Copyright (c) 2015 Steven Fackler

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

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
- Modifications: integrated into the Bun workspace; rewrote product, storage,
  privacy, platform, and AI claims for the current Noura architecture; retained
  the layout, responsive styling, and WebGL shader treatment
- Notice obligation: retain the predecessor MIT notice below
- Review: source and license inspected by the migration assistant; migration requested by Raphael on 2026-09-11
- Migration date: 2026-09-11

The copied source came from a local predecessor of Noura, not directly from an
external donor repository. The predecessor directory does not contain `.git`,
so a commit identifier cannot be recovered. The hashes above pin the exact
material reviewed for this migration.

### Predecessor license notice

MIT License

Copyright (c) 2018 Johannes Millan

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
