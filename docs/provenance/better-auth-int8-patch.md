# Better Auth `int8` type-match patch

- Repository: https://github.com/better-auth/better-auth
- Package: `better-auth@1.7.2`, pinned in `bun.lock`
- Source file: `dist/db/get-migration.mjs`
- Applicable license: MIT. Better Auth is consumed as a dependency; this is a minimal patch to a published build artifact, not a copied implementation.
- Destination: `patches/better-auth@1.7.2.patch`, applied by Bun `patchedDependencies` in the root `package.json`.
- Modifications: `matchType` compares a database column's `dataType` against a per-driver allowlist of type names. The Postgres `number` allowlist contains `integer` and `bigint` but not the internal Postgres type names `int2`/`int8` (nor `float4`/`float8`), so a `bigint` column is reported as `int8` and fails the match. The patch adds those internal aliases to the Postgres `number` allowlist. No behavior changes beyond suppressing the false type-mismatch warning; the declared field is already `bigint: true`.
- Why the patch instead of a config change: `rateLimit.lastRequest` stores `Date.now()` epoch milliseconds. Better Auth's schema already marks the field `bigint: true`, so the database column is correctly `int8`; an `integer` column would overflow. The warning came only from the alias gap above.
- Upstream basis: the get-tables definition at `@better-auth/core@1.7.2` `src/db/schema/rate-limit.ts` and `src/db/get-tables.ts`, which declare `lastRequest` as `{ type: "number", bigint: true }`.
- Notice obligations: none beyond the dependency's MIT terms already tracked for `better-auth` in the server license report.
- Reviewer: Codex
- Date: 2026-09-16
