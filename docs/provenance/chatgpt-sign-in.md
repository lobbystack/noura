# Sign in with ChatGPT: OpenAI's documentation and OpenCode as reference

Continue with ChatGPT follows OpenAI's documentation for open-source apps that use a ChatGPT plan. Noura copied no source: `crates/local-core/src/ai/chatgpt.rs`, `chatgpt_stream.rs`, `chatgpt_fake.rs` and `crates/app-core/src/ai/chatgpt.rs` are new code. This record names what each source contributed, so a later change that does copy code starts from a verified source.

## OpenAI's documentation

- Pages, read on 2026-11-27: https://developers.openai.com/siwc/token-sharing-open-source, `/sign-in`, `/profiles-and-sessions`, `/token-reference`, `/models-and-inference`, `/errors-and-recovery`, `/preview-limitations`, and https://developers.openai.com/siwc/ui-ux-guidelines
- Followed: registration with `client_id=dynamic_agent_client`, `agent_name_hint` and a persistent `ext_agent_host_id`; the issued client ID for later sign-ins with `login_hint`; a loopback callback on `127.0.0.1` at `/auth/callback`; PKCE S256 with a fresh `state` and `nonce`; the token exchange with `resource=https://api.openai.com/v1` and no secret; ID token checks against the published keys; the `chatgpt.tokens.use.direct` scope check; refresh with rotation, serialized per account; revocation through `revocation_endpoint` on sign-out; Responses requests with `store: false`, `stream: true`, `instructions` in place of system items, and tools grouped in a namespace; the labels Continue with ChatGPT, Using ChatGPT plan and Manage usage, and the one-time notice.

## OpenCode

- Repository: https://github.com/anomalyco/opencode
- Commit: `7bfda96047aea7dd28fc61dbfa7c69bea670849e` (2026-11-26)
- License: MIT (`LICENSE`), Copyright (c) 2025 opencode. The files read carry no headers of their own.
- Read: `packages/core/src/plugin/provider/chatgpt.ts` and `packages/core/src/plugin/provider/openai.ts`
- Borrowed: nothing beyond confirming that an open-source client follows OpenAI's documented flow as written. Noura doesn't use the Codex CLI's client ID or ChatGPT's `backend-api` endpoints, which `openai.ts` does, because OpenAI's documentation and support ask third-party apps not to.

## Dependency

The ID token's RS256 signature is checked with `ring` 0.17 (Apache-2.0 and ISC), which rustls already brings in. It is a dependency, not copied source, and the generated license file carries its license.

## Test key

`crates/local-core/tests/fixtures/chatgpt-test-signing-key.pk8` is a 2048-bit RSA key generated with `openssl genpkey` on 2026-11-27 for the fake authorization server in tests. It signs nothing outside those tests and no build that ships compiles it in.

Reviewed by the implementing agent on 2026-11-27 for the owner's approval.
