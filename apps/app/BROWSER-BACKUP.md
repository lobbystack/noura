# Browser backups

The browser shell downloads `.noura-backup.json` files locally. They are **not ZIP archives** and are not encrypted. No upload is involved. Store downloads privately; workspace files may contain sensitive content. Save or explicitly discard drafts before either action.

The version 1 UTF-8 JSON envelope has exactly these fields:

```json
{"format":"noura.browser-backup","version":1,"workspaceId":"<stable workspace ID>","entries":[{"path":"<relative file path>","base64":"<canonical padded RFC 4648 base64>"}]}
```

Each entry preserves its original bytes, including binary attachments and empty files. The codec only packages the client's snapshot; it does not parse or serialize canonical workspace files. The worker remains authoritative for manifest validation, managed object identities, and durable import. Derived indexes and internal storage journals are not backup content.

Import requires a fresh identity in browser storage. It does not replace or merge an existing workspace, nor rewrite its ID. To test restoration of an existing workspace, use a separate browser profile. Do not clear site data to bypass conflicts.

## Resource limits

- Selected backup: 96 MiB, checked before reading or parsing.
- Entries: 10,000; per-entry decoded bytes: 32 MiB; aggregate decoded bytes: 64 MiB.
- Portable relative paths use the shared storage validator (4,096 UTF-8 bytes maximum).
- All entry shapes, paths, canonical base64 and aggregate lengths are checked before allocating decoded binary arrays. JSON parsing itself allocates within the bounded input size; this is not a streaming format. Several in-memory copies may coexist.
- The underlying service allows 10,000 entries, 64 MiB per entry, and 512 MiB total. The UI intentionally uses lower memory limits. Export requests still obtain the client's structured snapshot before applying the UI limits.

The browser reports that a download was requested, not that the user saved it successfully. Object URLs are revoked after 60 seconds so download handling can start. Backups carry no authenticity signature or checksum; malformed canonical files are validated by the import service, but a well-formed modified backup is not authenticated.
