# PDF viewer dependencies

Reviewed by Codex on 2026-09-08 for the PDF viewing implementation.

## PDF.js

- Repository: https://github.com/mozilla/pdf.js
- Exact commit: `1c8020a7d4e43668ac287a3ecf9a8dbea17e4c56` (PDF.js 6.3.289).
- Distribution: pinned `pdfjs-dist@6.3.289`, integrity recorded in `bun.lock`.
- Source paths: `src/`, `web/`, `external/bcmaps/`, `external/standard_fonts/`, `external/openjpeg/`, `external/qcms/`; consumed through the published `build/`, `web/`, `cmaps/`, `standard_fonts/`, `wasm/`, and `iccs/` artifacts.
- Destination: app dependency bundle and generated `pdfjs/` static assets, controlled by `apps/app/pdf-assets.ts`.
- Modifications: no donor JavaScript or CSS source copied or adapted. Package imports and binary assets are used verbatim, except that unused QuickJS scripting assets and old Liberation fonts are excluded and replaced as described below.
- Licenses: Apache-2.0 for PDF.js; BSD-3-Clause for Adobe CMaps, PDFium/Foxit fonts, and the PDFium JBIG2 decoder; BSD-2-Clause for OpenJPEG; MIT for qcms; CC0-1.0 for ICC profiles. Each asset directory's supplied license files are emitted with the assets.
- Notices: retain the package license, embedded source headers, and each asset license; see `THIRD_PARTY_NOTICES.md`.

## Liberation Sans 2.1.5

- Repository: https://github.com/liberationfonts/liberation-fonts
- Exact commit: `4b0192046158094654e865245832c66d2104219e` (tag `2.1.5`).
- Official release: https://github.com/liberationfonts/liberation-fonts/releases/tag/2.1.5
- Release archive: https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz
- Source paths: release archive `liberation-fonts-ttf-2.1.5/LiberationSans-{Regular,Bold,Italic,BoldItalic}.ttf` and `LICENSE`; built upstream from `src/` at the commit above.
- Destination: `apps/app/vendor/pdf-fonts/`, emitted as `pdfjs/standard_fonts/`.
- Applicable license: SIL Open Font License 1.1. Copyright 2010 Google Corporation and 2012 Red Hat, Inc.; reserved names apply.
- Modifications: none. Four font binaries and the license are copied verbatim. These replace PDF.js's older GPL-with-font-exception Liberation binaries; those older files and their license are not shipped.
- Notice obligations: retain the complete OFL license and copyright statements, do not sell fonts alone or rename modified fonts to reserved names. Fonts remain unmodified.

Archive SHA-256: `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`.
