//! Byte layout of raw Markdown files: the BOM and each line's ending. Edits
//! work on LF text, and saves restore the original layout so unchanged lines
//! keep their own endings.

use thiserror::Error;

/// The raw Markdown bytes are not valid UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("The raw Markdown file is not UTF-8")]
pub struct InvalidUtf8;

/// How a raw Markdown file was encoded: its BOM and the ending of each line.
/// Saves write unchanged lines back with their own ending, so an edit never
/// rewrites the endings of lines it did not touch.
#[derive(Debug, Clone)]
pub struct RawLayout {
    /// The file's text with every CRLF turned into LF.
    body: String,
    /// For each line of `body` (as split by `split_inclusive('\n')`), whether
    /// it ended in CRLF on disk.
    crlf: Vec<bool>,
    /// The ending most lines use, for lines an edit adds. Ties go to LF.
    pub uses_crlf: bool,
    pub has_bom: bool,
}

impl RawLayout {
    /// A layout where every line ends the same way.
    pub fn uniform(body: &str, uses_crlf: bool, has_bom: bool) -> Self {
        Self {
            body: body.to_owned(),
            crlf: body.split_inclusive('\n').map(|_| uses_crlf).collect(),
            uses_crlf,
            has_bom,
        }
    }

    /// Whether any line ended in CRLF.
    pub fn any_crlf(&self) -> bool {
        self.crlf.iter().any(|crlf| *crlf)
    }
}

/// Encode `body` (LF text) with the BOM and line endings of `layout`.
pub fn compose_raw_bytes(body: &str, layout: &RawLayout) -> Vec<u8> {
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

/// Decode raw Markdown bytes into LF text and the layout needed to write it
/// back byte for byte.
pub fn split_raw_bytes(bytes: &[u8]) -> Result<(String, RawLayout), InvalidUtf8> {
    let (has_bom, text_bytes) = if bytes.starts_with(b"\xEF\xBB\xBF") {
        (true, &bytes[3..])
    } else {
        (false, bytes)
    };
    let text = std::str::from_utf8(text_bytes).map_err(|_| InvalidUtf8)?;
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

/// The history directory segment for a raw Markdown file, derived from its
/// workspace-relative path.
pub fn raw_history_dir(relative: &str) -> String {
    let digest = blake3::hash(relative.as_bytes()).to_hex();
    format!("raw-{}", &digest[..16])
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

    #[test]
    fn split_reports_bom_and_rejects_invalid_utf8() {
        let (body, layout) = split_raw_bytes(b"\xEF\xBB\xBFa\r\nb\r\n").unwrap();
        assert_eq!(body, "a\nb\n");
        assert!(layout.has_bom && layout.uses_crlf);
        assert_eq!(split_raw_bytes(b"\xFF\xFE").unwrap_err(), InvalidUtf8);
    }

    #[test]
    fn uniform_layout_applies_one_ending_to_every_line() {
        let layout = RawLayout::uniform("a\nb\n", true, true);
        assert_eq!(
            compose_raw_bytes("a\nb\nc\n", &layout),
            b"\xEF\xBB\xBFa\r\nb\r\nc\r\n"
        );
    }

    #[test]
    fn raw_history_dir_is_a_stable_path_digest() {
        let segment = raw_history_dir("notes/raw.md");
        assert_eq!(segment.len(), "raw-".len() + 16);
        assert!(segment.starts_with("raw-"));
        assert_eq!(segment, raw_history_dir("notes/raw.md"));
        assert_ne!(segment, raw_history_dir("notes/other.md"));
        assert_eq!(
            segment,
            format!("raw-{}", &blake3::hash(b"notes/raw.md").to_hex()[..16])
        );
    }
}
