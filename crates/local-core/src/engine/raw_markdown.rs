//! Markdown files without a managed ID, edited as raw text. Saves keep the
//! bytes the editor did not touch, including line endings and the BOM.

use super::*;
pub(super) use workspace_format::raw_text::{RawLayout, compose_raw_bytes, raw_history_dir};

/// Complete current contents of one Markdown file addressed by relative path.
/// Raw files expose their full bytes as UTF-8 text with CRLF normalized to LF.
/// Saves keep the BOM and each unchanged line's own ending; `uses_crlf` is
/// the ending most lines use, which new lines get.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawMarkdownRead {
    pub relative_path: String,
    pub body: String,
    pub revision: String,
    pub uses_crlf: bool,
    pub has_bom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawReconcileInput {
    pub relative_path: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RawReconcileResult {
    Unchanged { current: RawMarkdownRead },
    Merged { current: RawMarkdownRead },
    Conflict { current: RawMarkdownRead },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawSaveInput {
    pub relative_path: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RawSaveResult {
    Saved {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
    Merged {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
    Conflict {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawConflictResolveInput {
    pub relative_path: String,
    pub current_revision: String,
    pub local_body: String,
    pub resolution: ConflictResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawConflictResolveResult {
    pub current: RawMarkdownRead,
    #[ts(rename = "managedObject")]
    pub managed_object: Option<WorkspaceObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MarkdownLinkTarget {
    Managed {
        object: WorkspaceObject,
    },
    Markdown {
        document: RawMarkdownRead,
    },
    Pdf {
        #[serde(rename = "relativePath")]
        #[ts(rename = "relativePath")]
        relative_path: String,
        page: Option<u32>,
    },
    Asset {
        #[ts(rename = "relativePath")]
        relative_path: String,
    },
    Unresolved,
}

impl WorkspaceEngine {
    // --- Raw Markdown (unmanaged and malformed files) ---

    /// Read one Markdown file addressed by relative path. Managed frontmatter
    /// is returned as-is; the editor owns complete raw contents.
    pub fn read_raw_markdown(&self, relative_path: &str) -> Result<RawMarkdownRead> {
        let path = validate_raw_markdown_path(&self.root, relative_path)?;
        let bytes = std::fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound && icloud_placeholder(&path).exists() {
                let mut error = CoreError::new(
                    "file_not_downloaded",
                    ErrorCategory::Filesystem,
                    "This file is still in iCloud. Download it in Finder, then open it again.",
                    "raw_markdown_read",
                );
                error.retryable = true;
                error.path = Some(relative_path.to_owned());
                return error;
            }
            CoreError::io(error, "raw_markdown_read", Some(relative_path))
        })?;
        let (body, layout) = split_raw_bytes(&bytes)?;
        Ok(RawMarkdownRead {
            relative_path: relative_path.to_owned(),
            body,
            revision: markdown::revision(&bytes),
            uses_crlf: layout.uses_crlf,
            has_bom: layout.has_bom,
        })
    }

    pub fn resolve_markdown_link(
        &self,
        source_relative_path: &str,
        target: &str,
    ) -> Result<MarkdownLinkTarget> {
        if target.starts_with("http://") || target.starts_with("https://") {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        // A target that only names an alias or heading has no file behind it
        // to resolve; callers represent it as an ordinary unresolved link.
        if markdown_target_path(target, true).is_empty() {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        let mut relative = resolve_markdown_target(source_relative_path, target, true)?;
        let mut path = resolve_for_write(&self.root, &relative, "markdown_link_resolve")?;
        if !path.exists() && Path::new(&relative).extension().is_none() {
            relative.push_str(".md");
            path = resolve_for_write(&self.root, &relative, "markdown_link_resolve")?;
        }
        if !path.exists() || !path.is_file() {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        if relative.to_ascii_lowercase().ends_with(".md") {
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "markdown_link_resolve", Some(&relative)))?;
            if let ParsedMarkdown::Managed(object) = markdown::parse_markdown(&relative, &bytes) {
                return Ok(MarkdownLinkTarget::Managed { object });
            }
            return self
                .read_raw_markdown(&relative)
                .map(|document| MarkdownLinkTarget::Markdown { document });
        }
        if relative.to_ascii_lowercase().ends_with(".pdf") {
            return Ok(MarkdownLinkTarget::Pdf {
                relative_path: relative,
                page: target
                    .split('|')
                    .next()
                    .and_then(|value| value.split_once('#'))
                    .and_then(|(_, fragment)| fragment.strip_prefix("page="))
                    .and_then(|page| page.parse::<u32>().ok())
                    .filter(|page| *page > 0),
            });
        }
        Ok(MarkdownLinkTarget::Asset {
            relative_path: relative,
        })
    }

    pub(super) fn reindex_raw_markdown(
        &self,
        relative: &str,
        bytes: &[u8],
        operation: &str,
    ) -> Result<IndexStatus> {
        let destination = resolve_for_write(&self.root, relative, operation)?;
        let parsed = markdown::parse_markdown(relative, bytes);
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(relative, bytes, mtime_ns(&destination), &parsed)
            });
        let (index_status, _) = self.index_outcome(result);
        Ok(index_status)
    }

    /// Reconcile a raw Markdown draft against the canonical file without
    /// writing. Bodies merge line-by-line; every overlap requires review.
    pub fn reconcile_raw_markdown(&self, input: RawReconcileInput) -> Result<RawReconcileResult> {
        let path = validate_raw_markdown_path(&self.root, &input.relative_path)?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_reconcile",
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            CoreError::io(error, "raw_markdown_reconcile", Some(&input.relative_path))
        })?;
        let (external_body, layout) = split_raw_bytes(&bytes)?;
        let (uses_crlf, has_bom) = (layout.uses_crlf, layout.has_bom);
        let current = RawMarkdownRead {
            relative_path: input.relative_path.clone(),
            body: external_body.clone(),
            revision: markdown::revision(&bytes),
            uses_crlf,
            has_bom,
        };
        if current.revision == input.base_revision {
            return Ok(RawReconcileResult::Unchanged { current });
        }
        match Self::merge_raw_body(&input, &external_body) {
            Some(merged) => Ok(RawReconcileResult::Merged {
                current: RawMarkdownRead {
                    body: merged,
                    ..current
                },
            }),
            None => Ok(RawReconcileResult::Conflict { current }),
        }
    }

    fn merge_raw_body(input: &RawReconcileInput, external_body: &str) -> Option<String> {
        merge_markdown_text(&input.base_body, &input.local_body, external_body)
    }

    /// Reconcile and durably commit a raw Markdown draft. A clean base writes
    /// the local body; a changed base merges line-by-line and snapshots the
    /// displaced external version. CRLF and BOM conventions are preserved.
    pub fn save_raw_markdown(&self, input: RawSaveInput) -> Result<RawSaveResult> {
        let relative = crate::path::validate_relative(&input.relative_path, "raw_markdown_save")?
            .to_str()
            .ok_or_else(|| {
                CoreError::validation(
                    "non_utf8_path",
                    "The raw Markdown path is not UTF-8",
                    "raw_markdown_save",
                )
            })?
            .replace('\\', "/");
        let path = validate_raw_markdown_path(&self.root, &relative)?;
        let _guard = self.write_lock("raw_markdown_save")?;
        self.collaboration_guard_file_mutation(&relative)?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_save",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_markdown_save", Some(&relative)))?;
        let (external_body, layout) = split_raw_bytes(&bytes)?;
        let (uses_crlf, has_bom) = (layout.uses_crlf, layout.has_bom);
        let current = RawMarkdownRead {
            relative_path: relative.clone(),
            body: external_body.clone(),
            revision: markdown::revision(&bytes),
            uses_crlf,
            has_bom,
        };
        let merged_body = if current.revision == input.base_revision {
            input.local_body.clone()
        } else {
            match merge_markdown_text(&input.base_body, &input.local_body, &external_body) {
                Some(merged) => {
                    let segment = raw_history_dir(&relative);
                    write_snapshot(
                        &self.root,
                        "raw_markdown_save",
                        &segment,
                        "external",
                        &bytes,
                    )?;
                    merged
                }
                None => {
                    return Ok(RawSaveResult::Conflict {
                        current,
                        managed_object: None,
                    });
                }
            }
        };
        let next_bytes = compose_raw_bytes(&merged_body, &layout);
        let next_relative = PathBuf::from(&relative);
        atomic_write_checked(
            &self.root,
            &next_relative,
            &next_bytes,
            Some(&current.revision),
            "raw_markdown_save",
        )?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(relative.clone(), markdown::revision(&next_bytes));
        }
        self.reindex_raw_markdown(&relative, &next_bytes, "raw_markdown_save")?;
        self.emit(
            "file:changed",
            EventSource::Application,
            serde_json::json!({ "paths": [relative] }),
        );
        self.emit(
            "search:index-updated",
            EventSource::Application,
            serde_json::json!({}),
        );
        let managed_object = match markdown::parse_markdown(&relative, &next_bytes) {
            ParsedMarkdown::Managed(object) => Some(object),
            _ => None,
        };
        Ok(RawSaveResult::Saved {
            current: RawMarkdownRead {
                relative_path: relative,
                body: merged_body,
                revision: markdown::revision(&next_bytes),
                uses_crlf,
                has_bom,
            },
            managed_object,
        })
    }

    /// Adopt the external file version or replace it with the reviewed local
    /// draft. Both directions snapshot the version they displace first.
    pub fn resolve_raw_conflict(
        &self,
        input: RawConflictResolveInput,
    ) -> Result<RawConflictResolveResult> {
        let relative =
            crate::path::validate_relative(&input.relative_path, "raw_markdown_resolve")?
                .to_str()
                .ok_or_else(|| {
                    CoreError::validation(
                        "non_utf8_path",
                        "The raw Markdown path is not UTF-8",
                        "raw_markdown_resolve",
                    )
                })?
                .replace('\\', "/");
        let path = validate_raw_markdown_path(&self.root, &relative)?;
        let _guard = self.write_lock("raw_markdown_resolve")?;
        self.collaboration_guard_file_mutation(&relative)?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_resolve",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_markdown_resolve", Some(&relative)))?;
        if markdown::revision(&bytes) != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "raw_markdown_resolve",
            );
            error.details = Some(serde_json::json!({
                "currentRevision": markdown::revision(&bytes),
            }));
            return Err(error);
        }
        let (external_body, layout) = split_raw_bytes(&bytes)?;
        let (uses_crlf, has_bom) = (layout.uses_crlf, layout.has_bom);
        match input.resolution {
            ConflictResolution::UseExternal => {
                let segment = raw_history_dir(&relative);
                let local_bytes = compose_raw_bytes(&input.local_body, &layout);
                write_snapshot(
                    &self.root,
                    "raw_markdown_resolve",
                    &segment,
                    "local",
                    &local_bytes,
                )?;
                let managed_object = match markdown::parse_markdown(&relative, &bytes) {
                    ParsedMarkdown::Managed(object) => Some(object),
                    _ => None,
                };
                Ok(RawConflictResolveResult {
                    current: RawMarkdownRead {
                        relative_path: relative,
                        body: external_body,
                        revision: markdown::revision(&bytes),
                        uses_crlf,
                        has_bom,
                    },
                    managed_object,
                })
            }
            ConflictResolution::ReplaceExternal => {
                let segment = raw_history_dir(&relative);
                write_snapshot(
                    &self.root,
                    "raw_markdown_resolve",
                    &segment,
                    "external",
                    &bytes,
                )?;
                let next_bytes = compose_raw_bytes(&input.local_body, &layout);
                let next_relative = PathBuf::from(&relative);
                atomic_write_checked(
                    &self.root,
                    &next_relative,
                    &next_bytes,
                    Some(&input.current_revision),
                    "raw_markdown_resolve",
                )?;
                if let Ok(mut journal) = self.self_writes.lock() {
                    journal.insert(relative.clone(), markdown::revision(&next_bytes));
                }
                self.reindex_raw_markdown(&relative, &next_bytes, "raw_markdown_resolve")?;
                self.emit(
                    "file:changed",
                    EventSource::Application,
                    serde_json::json!({ "paths": [relative] }),
                );
                self.emit(
                    "search:index-updated",
                    EventSource::Application,
                    serde_json::json!({}),
                );
                let managed_object = match markdown::parse_markdown(&relative, &next_bytes) {
                    ParsedMarkdown::Managed(object) => Some(object),
                    _ => None,
                };
                Ok(RawConflictResolveResult {
                    current: RawMarkdownRead {
                        relative_path: relative,
                        body: input.local_body,
                        revision: markdown::revision(&next_bytes),
                        uses_crlf,
                        has_bom,
                    },
                    managed_object,
                })
            }
        }
    }
}

pub(super) fn split_raw_bytes(bytes: &[u8]) -> Result<(String, RawLayout)> {
    workspace_format::raw_text::split_raw_bytes(bytes).map_err(|_| {
        CoreError::new(
            "invalid_utf8",
            ErrorCategory::Parse,
            "The raw Markdown file is not UTF-8",
            "raw_markdown_read",
        )
    })
}

fn validate_raw_markdown_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if !relative.to_ascii_lowercase().ends_with(".md") {
        return Err(CoreError::validation(
            "invalid_raw_markdown_path",
            "Raw edits are limited to Markdown files",
            "raw_markdown",
        ));
    }
    resolve_for_write(root, relative, "raw_markdown")
}

/// Strip the alias (`|`) and — when fragments are meaningful — the heading
/// fragment (`#`) from an Obsidian-style target, leaving the resolvable
/// path. An empty result means the target named only an alias or fragment.
fn markdown_target_path(target: &str, allow_fragment: bool) -> &str {
    let path = target.split('|').next().unwrap_or_default().trim();
    if allow_fragment {
        path.split('#').next().unwrap_or_default()
    } else {
        path
    }
}

pub(super) fn resolve_markdown_target(
    source_relative_path: &str,
    target: &str,
    allow_fragment: bool,
) -> Result<String> {
    let target = markdown_target_path(target, allow_fragment);
    if target.is_empty() || target.contains('\0') {
        return Err(CoreError::validation(
            "invalid_markdown_target",
            "The Markdown target is empty or invalid",
            "markdown_target_resolve",
        ));
    }
    let decoded = crate::pdf::decode_target(target);
    let target = match &decoded {
        Ok(decoded) if decoded.to_ascii_lowercase().ends_with(".pdf") => decoded.as_str(),
        Err(error) if target.to_ascii_lowercase().ends_with(".pdf") => return Err(error.clone()),
        _ => target,
    };
    let source = crate::path::validate_relative(source_relative_path, "markdown_target_resolve")?;
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let joined = parent.join(target);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(CoreError::validation(
                        "path_traversal",
                        "The Markdown target escapes the workspace",
                        "markdown_target_resolve",
                    ));
                }
            }
            _ => {
                return Err(CoreError::validation(
                    "invalid_markdown_target",
                    "The Markdown target must be a relative workspace path",
                    "markdown_target_resolve",
                ));
            }
        }
    }
    normalized
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The Markdown target path is not UTF-8",
                "markdown_target_resolve",
            )
        })
}

fn write_snapshot(
    root: &Path,
    operation: &str,
    segment: &str,
    kind: &str,
    bytes: &[u8],
) -> Result<()> {
    let revision = markdown::revision(bytes);
    let relative_path = PathBuf::from(".noura")
        .join("history")
        .join(segment)
        .join(format!("{revision}-{kind}.md"));
    let relative = relative_path.to_str().ok_or_else(|| {
        CoreError::validation(
            "non_utf8_path",
            "The recovery snapshot path is not UTF-8",
            "history_snapshot",
        )
    })?;
    // This helper always writes the internal history directory, so it validates
    // against the internal operation even when the caller is a user-facing one.
    let destination = resolve_for_write(root, relative, "history_snapshot")?;
    if destination.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    }
    atomic_write(root, &relative_path, bytes, operation)
}
