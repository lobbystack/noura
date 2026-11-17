//! The Transcription's body: your notes, then the `## Summary` section
//! Noura adds below them. Summarizing again replaces only that section,
//! so every byte of your notes stays as you wrote it.

/// The heading the Summary's section starts with.
pub(crate) const SUMMARY_HEADING: &str = "## Summary";

/// The level of the ATX heading `line` starts, if it is one.
fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.bytes().take_while(|&b| b == b'#').count();
    let after = &trimmed[level..];
    ((1..=6).contains(&level) && (after.is_empty() || after.starts_with([' ', '\t'])))
        .then_some(level)
}

/// Each line of `text` with its byte offset and whether it sits inside a
/// fenced code block, where a `#` starts no heading.
fn lines(text: &str) -> Vec<(usize, &str, bool)> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut fence: Option<&str> = None;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| trimmed.starts_with(marker));
        let inside = fence.is_some();
        match (fence, marker) {
            (None, Some(marker)) => fence = Some(marker),
            (Some(open), Some(marker)) if open == marker => fence = None,
            _ => {}
        }
        out.push((offset, line, inside || marker.is_some()));
        offset += raw.len();
    }
    out
}

/// Where the Summary's section starts and ends in `body`: from its last
/// `## Summary` line to the next heading of level 1 or 2, or the end.
pub(crate) fn summary_span(body: &str) -> Option<(usize, usize)> {
    let lines = lines(body);
    let start = lines
        .iter()
        .rposition(|(_, line, code)| !code && line.trim_end() == SUMMARY_HEADING)?;
    let end = lines[start + 1..]
        .iter()
        .find(|(_, line, code)| !code && heading_level(line).is_some_and(|level| level <= 2))
        .map_or(body.len(), |(offset, ..)| *offset);
    Some((lines[start].0, end))
}

/// Your notes: the body without the Summary's section.
pub(crate) fn notes(body: &str) -> String {
    match summary_span(body) {
        Some((start, end)) => format!("{}{}", &body[..start], &body[end..]),
        None => body.to_owned(),
    }
}

/// The Summary's text, as the body holds it now.
pub(crate) fn summary(body: &str) -> Option<&str> {
    summary_span(body).map(|(start, end)| {
        body[start..end]
            .strip_prefix(SUMMARY_HEADING)
            .unwrap_or_default()
            .trim()
    })
}

/// `markdown` with every heading two levels deeper, at most level 6, so
/// the model's `# Next Steps` sits inside the Summary's section.
pub(crate) fn demote(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 16);
    for (_, line, code) in lines(markdown) {
        match heading_level(line) {
            Some(level) if !code => {
                let trimmed = line.trim_start_matches(' ');
                let deeper = (level + 2).min(6);
                out.push_str(&"#".repeat(deeper));
                out.push_str(&trimmed[level..]);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// `body` with `summary` as its Summary's section: in place of the old
/// one, or added below your notes. The bytes before the section, and
/// any after it, stay as they were.
pub(crate) fn with_summary(body: &str, summary: &str) -> String {
    let section = format!(
        "{SUMMARY_HEADING}\n\n{}\n",
        demote(summary.trim()).trim_end()
    );
    match summary_span(body) {
        Some((start, end)) => {
            let rest = &body[end..];
            let gap = if rest.is_empty() { "" } else { "\n" };
            format!("{}{section}{gap}{rest}", &body[..start])
        }
        None if body.is_empty() => section,
        None if body.ends_with("\n\n") => format!("{body}{section}"),
        None if body.ends_with('\n') => format!("{body}\n{section}"),
        None => format!("{body}\n\n{section}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_goes_below_your_notes_which_keep_their_bytes() {
        let notes = "Agenda\r\n- pricing  \n\n### Must keep\nno newline at end";
        let body = with_summary(
            notes,
            "# Decisions\n- Ship Friday\n\n# Next Steps\n- Ada: notes",
        );
        assert!(body.starts_with(notes), "{body:?}");
        assert_eq!(
            &body[notes.len()..],
            "\n\n## Summary\n\n### Decisions\n- Ship Friday\n\n### Next Steps\n- Ada: notes\n"
        );
        assert_eq!(super::notes(&body), format!("{notes}\n\n"));
        assert_eq!(
            summary(&body),
            Some("### Decisions\n- Ship Friday\n\n### Next Steps\n- Ada: notes")
        );
        assert_eq!(with_summary("", "Short."), "## Summary\n\nShort.\n");
        assert_eq!(
            with_summary("Notes\n", "Short."),
            "Notes\n\n## Summary\n\nShort.\n"
        );
    }

    #[test]
    fn summarizing_again_replaces_only_the_summary() {
        let first = with_summary("Notes\n", "Old.");
        let second = with_summary(&first, "# New\n- point");
        assert_eq!(second, "Notes\n\n## Summary\n\n### New\n- point\n");
        // Notes you added after the Summary, under their own heading, stay.
        let edited = format!("{first}\n## Follow-up\nCall Ada.\n");
        let again = with_summary(&edited, "Newer.");
        assert_eq!(
            again,
            "Notes\n\n## Summary\n\nNewer.\n\n## Follow-up\nCall Ada.\n"
        );
        assert_eq!(summary("No summary."), None);
    }

    #[test]
    fn a_summary_heading_inside_code_is_text() {
        let body = "```\n## Summary\n```\n";
        assert_eq!(summary_span(body), None);
        let demoted = demote("```\n# not a heading\n```\n# Heading\n#tag\n");
        assert_eq!(demoted, "```\n# not a heading\n```\n### Heading\n#tag\n");
        assert_eq!(demote("##### Deep"), "###### Deep\n");
    }
}
