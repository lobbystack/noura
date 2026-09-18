# Noura documentation

Use these guides to run Noura, understand its file-backed workspace, and contribute to its plugins and services. Start with the guide for your task:

- **Run Noura**: follow the [desktop setup instructions](../README.md#try-noura)
- **Contribute code**: read the [contributor conventions](../AGENTS.md) and [desktop verification guide](testing/local-alpha-acceptance.md)
- **Work with plugins**: learn the [activation lifecycle and capability contracts](architecture/plugin-runtime.md)
- **Understand workspace files**: read the [workspace format](workspace-format/v1.md) and [local-core architecture](architecture/local-core.md)
- **Understand AI access**: read about [provider consent, tool approval, and chat persistence](architecture/ai-runtime.md)
- **Run the sync service**: follow [server setup](../apps/server/README.md), then the [operations guide](../apps/server/OPERATIONS.md)

## Workspace format references

Treat these specifications as the public contract for durable workspace data. Rust owns parsing and deterministic serialization; TypeScript validators use the same [conformance fixtures](workspace-format/fixtures/):

- [Workspace format v1](workspace-format/v1.md): manifests, managed Markdown, stable identifiers, and chats
- [Encrypted sync protocol](workspace-format/sync-v1.md): signed operations, key envelopes, file changes, and attachments
- [Collaboration protocol](workspace-format/collaboration.md): text generations, updates, and recovery
- [Encrypted checkpoints](workspace-format/checkpoints.md): snapshots and access transitions

## Architecture references

Use these documents to understand component responsibilities and the boundaries a change must preserve:

- [Local core](architecture/local-core.md)
- [Plugin runtime](architecture/plugin-runtime.md)
- [AI runtime](architecture/ai-runtime.md)
- [Encrypted synchronization service](architecture/sync-server.md)

## Security and provenance

Follow the [security policy](../SECURITY.md) to report a vulnerability. Review the relevant boundaries and dependency requirements before changing them:

- [Threat model](security/threat-model.md)
- [Dependency audit](security/dependency-audit.md)
- [Source provenance requirements](provenance/README.md)
- [Third-party notices](../THIRD_PARTY_NOTICES.md)
