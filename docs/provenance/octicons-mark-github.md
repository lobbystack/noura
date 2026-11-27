# Octicons GitHub mark

The marketing site draws the GitHub logo next to its GitHub links with the `mark-github` icon from Primer Octicons.

## Source

- Repository: https://github.com/primer/octicons
- Tag: `v19.33.0`, commit `cc4e12df6ff8292447ba9141eaa2a6f6e1c59a85`
- Source path: `icons/mark-github-24.svg`, the `d` attribute of its only `<path>`
- SHA-256 of the source file: `bce494189797623c34e39a41c2c38a132bdca23ce4e6ac06b70116ed7e91ce26`
- License: MIT, Copyright (c) 2026 GitHub Inc., from the repository's `LICENSE` at that commit

## Destination

`apps/website/src/lib/components/Icon.svelte`, the `github` entry of `paths`.

## Modifications

None. The path string is copied as it is. The source SVG uses a 24-unit viewBox (`0 0 24 24`), the same one `Icon.svelte` renders every icon in, and the site fills the path with the current text color.

## Notices

`THIRD_PARTY_NOTICES.md` carries the MIT notice. The mark is also a GitHub trademark: the site uses it only to link to noura's GitHub repository, as GitHub's logo guidelines allow.

## Review

Reviewed by Claude for the repository owner on 2026-11-26.
