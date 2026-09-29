//! Markdown files without a managed ID, edited as raw text. Saves keep the
//! bytes the editor did not touch, including line endings and the BOM.

use super::*;

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

/// How a raw Markdown file was encoded: its BOM and the ending of each line.
/// Saves write unchanged lines back with their own ending, so an edit never
/// rewrites the endings of lines it did not touch.
#[derive(Debug, Clone)]
pub(super) struct RawLayout {
    /// The file's text with every CRLF turned into LF.
    body: String,
    /// For each line of `body` (as split by `split_inclusive('\n')`), whether
    /// it ended in CRLF on disk.
    crlf: Vec<bool>,
    /// The ending most lines use, for lines an edit adds. Ties go to LF.
    uses_crlf: bool,
    pub(super) has_bom: bool,
}

impl RawLayout {
    /// A layout where every line ends the same way.
    pub(super) fn uniform(body: &str, uses_crlf: bool, has_bom: bool) -> Self {
        Self {
            body: body.to_owned(),
            crlf: body.split_inclusive('\n').map(|_| uses_crlf).collect(),
            uses_crlf,
            has_bom,
        }
    }

    /// Whether any line ended in CRLF.
    pub(super) fn any_crlf(&self) -> bool {
        self.crlf.iter().any(|crlf| *crlf)
    }
}

pub(super) fn compose_raw_bytes(body: &str, layout: &RawLayout) -> Vec<u8> {
    let mut bytes = if layout.has_bom {
        b"\xEF\xBB\xBF".to_vec()
    } else {
        Vec::new()
    };
    let any_crlf = layout.any_crlf();
    let all_crlf = layout
        .body
        .split_inclusive('\n')
        .zip(&layout.crlf)
        .all(|(line, crlf)| *crlf || !line.ends_with('\n'));
    if !any_crlf {
        // Every line ends in LF, and so do new ones.
        bytes.extend_from_slice(body.as_bytes());
        return bytes;
    }
    if all_crlf && layout.uses_crlf {
        bytes.extend_from_slice(body.replace('\n', "\r\n").as_bytes());
        return bytes;
    }
    let lines = body.split_inclusive('\n').collect::<Vec<_>>();
    let matches = match_unchanged_lines(&layout.body, &lines);
    for (line, original) in lines.into_iter().zip(matches) {
        match line.strip_suffix('\n') {
            Some(text) => {
                let crlf = original.map_or(layout.uses_crlf, |index| layout.crlf[index]);
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
            }
            None => bytes.extend_from_slice(line.as_bytes()),
        }
    }
    bytes
}

/// For each line of the new text, the index of the same unchanged line in
/// `original`, found with a line diff. Lines are compared with their LF.
fn match_unchanged_lines(original: &str, lines: &[&str]) -> Vec<Option<usize>> {
    let old = original.split_inclusive('\n').collect::<Vec<_>>();
    let mut matches = vec![None; lines.len()];
    // The shared start and end match line for line; only the middle is diffed.
    let prefix = old
        .iter()
        .zip(lines)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(lines[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    for (index, slot) in matches.iter_mut().enumerate().take(prefix) {
        *slot = Some(index);
    }
    for offset in 0..suffix {
        matches[lines.len() - 1 - offset] = Some(old.len() - 1 - offset);
    }
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &lines[prefix..lines.len() - suffix];
    if old_middle.is_empty() || new_middle.is_empty() {
        return matches;
    }
    let old_text = old_middle.concat();
    let new_text = new_middle.concat();
    // A context longer than both texts keeps the whole middle in one hunk,
    // so its lines list every old and new line in order.
    let mut options = diffy::DiffOptions::new();
    options.set_context_len(old_middle.len() + new_middle.len() + 1);
    let patch = options.create_patch(&old_text, &new_text);
    let mut paired = Vec::with_capacity(new_middle.len());
    let (mut old_index, mut new_index) = (0, 0);
    for hunk in patch.hunks() {
        for line in hunk.lines() {
            match line {
                diffy::Line::Context(_) => {
                    paired.push((new_index, old_index));
                    old_index += 1;
                    new_index += 1;
                }
                diffy::Line::Delete(_) => old_index += 1,
                diffy::Line::Insert(_) => new_index += 1,
            }
        }
    }
    // Trust the pairing only when it accounts for every line; otherwise the
    // middle lines take the file's usual ending.
    if patch.hunks().len() == 1 && old_index == old_middle.len() && new_index == new_middle.len() {
        for (new_line, old_line) in paired {
            matches[prefix + new_line] = Some(prefix + old_line);
        }
    }
    matches
}

pub(super) fn split_raw_bytes(bytes: &[u8]) -> Result<(String, RawLayout)> {
    let (has_bom, text_bytes) = if bytes.starts_with(b"\xEF\xBB\xBF") {
        (true, &bytes[3..])
    } else {
        (false, bytes)
    };
    let text = std::str::from_utf8(text_bytes).map_err(|_| {
        CoreError::new(
            "invalid_utf8",
            ErrorCategory::Parse,
            "The raw Markdown file is not UTF-8",
            "raw_markdown_read",
        )
    })?;
    let mut body = String::with_capacity(text.len());
    let mut crlf = Vec::new();
    let (mut crlf_lines, mut lf_lines) = (0usize, 0usize);
    for line in text.split_inclusive('\n') {
        match line.strip_suffix("\r\n") {
            Some(stripped) => {
                body.push_str(stripped);
                body.push('\n');
                crlf.push(true);
                crlf_lines += 1;
            }
            None => {
                body.push_str(line);
                crlf.push(false);
                if line.ends_with('\n') {
                    lf_lines += 1;
                }
            }
        }
    }
    let layout = RawLayout {
        body: body.clone(),
        crlf,
        uses_crlf: crlf_lines > lf_lines,
        has_bom,
    };
    Ok((body, layout))
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

fn raw_history_dir(relative: &str) -> String {
    let digest = blake3::hash(relative.as_bytes()).to_hex();
    format!("raw-{}", &digest[..16])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(bytes: &[u8]) -> Vec<u8> {
        let (body, layout) = split_raw_bytes(bytes).unwrap();
        compose_raw_bytes(&body, &layout)
    }

    fn edit(bytes: &[u8], change: impl FnOnce(&str) -> String) -> Vec<u8> {
        let (body, layout) = split_raw_bytes(bytes).unwrap();
        compose_raw_bytes(&change(&body), &layout)
    }

    #[test]
    fn unchanged_raw_text_round_trips_byte_for_byte() {
        for bytes in [
            &b""[..],
            b"no newline",
            b"lf\nonly\n",
            b"crlf\r\nonly\r\n",
            b"mixed\r\nendings\nhere\r\n",
            b"mixed\nendings\r\nno final newline",
            b"\xEF\xBB\xBFbom\r\nand lf\n",
            b"lone\rcarriage return\r\nthen lf\n",
            b"\r\n\n\r\n\n",
            b"trailing cr\r",
        ] {
            assert_eq!(
                round_trip(bytes),
                bytes,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn edits_keep_the_endings_of_lines_they_do_not_touch() {
        // Mostly CRLF: edited and new lines take CRLF, the LF line stays LF.
        let bytes = b"one\r\ntwo\nthree\r\nfour\r\n";
        assert_eq!(
            edit(bytes, |body| body.replace("three", "THREE")),
            b"one\r\ntwo\nTHREE\r\nfour\r\n"
        );
        assert_eq!(
            edit(bytes, |body| body.replace("two\n", "two\nadded\n")),
            b"one\r\ntwo\nadded\r\nthree\r\nfour\r\n"
        );
        assert_eq!(
            edit(bytes, |body| body.replace("one\n", "")),
            b"two\nthree\r\nfour\r\n"
        );
        // Mostly LF: new lines take LF, the CRLF line keeps CRLF.
        let bytes = b"a\nb\r\nc\nd\n";
        assert_eq!(
            edit(bytes, |body| format!("start\n{body}end\n")),
            b"start\na\nb\r\nc\nd\nend\n"
        );
        assert_eq!(
            edit(bytes, |body| body.replace("c\n", "c\nc\n")),
            b"a\nb\r\nc\nc\nd\n"
        );
    }

    #[test]
    fn uniform_files_keep_their_ending_for_new_lines() {
        assert_eq!(
            edit(b"a\r\nb\r\n", |body| format!("{body}c\n")),
            b"a\r\nb\r\nc\r\n"
        );
        assert_eq!(edit(b"a\nb\n", |body| format!("{body}c\n")), b"a\nb\nc\n");
        let (_, layout) = split_raw_bytes(b"a\r\nb\nc\n").unwrap();
        assert!(!layout.uses_crlf);
        assert!(layout.any_crlf());
    }
}
