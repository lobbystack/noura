# Third-Party Notices

Noura uses third-party dependencies under their respective licenses. The lockfiles and automated license report provide the dependency inventory.

The repository includes Apollo GraphQL's unmodified `rust-best-practices` agent skill under the MIT license. See `docs/provenance/apollo-rust-best-practices.md` for its pinned source and verification record.

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
