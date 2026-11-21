//! A task's body as the details pane edits it: a description, then the
//! task's subtasks as a Markdown checklist, the run of `- [ ]` and
//! `- [x]` lines that ends the body. The file stays ordinary Markdown,
//! and the task's fields don't change.
//!
//! Only that last run counts as subtasks, so a checklist in the middle of
//! the notes stays part of the description. Changing the description keeps
//! the checklist's bytes, and changing a subtask keeps the description's.

/// A subtask: a checklist line's text, and whether it's checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subtask {
    pub(crate) text: String,
    pub(crate) done: bool,
}

/// A body split into its description and its subtasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Body {
    /// Everything before the checklist, as the file holds it.
    head: String,
    /// The checklist's lines, as the file holds them.
    checklist: String,
    pub(crate) subtasks: Vec<Subtask>,
}

/// The subtask a checklist line holds.
fn subtask(line: &str) -> Option<Subtask> {
    let rest = line
        .trim_start()
        .strip_prefix("- [")
        .or_else(|| line.trim_start().strip_prefix("* ["))?;
    let (mark, text) = rest.split_at_checked(1)?;
    let text = text.strip_prefix("] ")?;
    let done = match mark {
        " " => false,
        "x" | "X" => true,
        _ => return None,
    };
    Some(Subtask {
        text: text.trim_end().to_owned(),
        done,
    })
}

impl Body {
    pub(crate) fn parse(body: &str) -> Self {
        // Byte offsets of each line's start, after its newline.
        let mut starts = vec![0];
        starts.extend(body.match_indices('\n').map(|(at, _)| at + 1));
        let lines: Vec<&str> = body.split_inclusive('\n').collect();
        let mut first = lines.len();
        // Blank lines after the checklist belong to it.
        while first > 0 && lines[first - 1].trim().is_empty() {
            first -= 1;
        }
        let end_of_items = first;
        while first > 0 && subtask(lines[first - 1]).is_some() {
            first -= 1;
        }
        if first == end_of_items {
            return Self {
                head: body.to_owned(),
                checklist: String::new(),
                subtasks: Vec::new(),
            };
        }
        let at = starts[first];
        Self {
            head: body[..at].to_owned(),
            checklist: body[at..].to_owned(),
            subtasks: lines[first..end_of_items]
                .iter()
                .filter_map(|line| subtask(line))
                .collect(),
        }
    }

    /// The description, without the blank lines around it.
    pub(crate) fn description(&self) -> &str {
        self.head.trim_matches(|c: char| c == '\n' || c == '\r')
    }

    /// The body with `description` in place of the description and the
    /// checklist as it was.
    pub(crate) fn with_description(&self, description: &str) -> String {
        let description = description.trim_end();
        match (description.is_empty(), self.checklist.is_empty()) {
            (true, _) => self.checklist.clone(),
            (false, true) => format!("{description}\n"),
            (false, false) => format!("{description}\n\n{}", self.checklist),
        }
    }

    /// The body with `subtasks` as its checklist and the description as
    /// it was.
    pub(crate) fn with_subtasks(&self, subtasks: &[Subtask]) -> String {
        let checklist: String = subtasks
            .iter()
            .map(|subtask| {
                let mark = if subtask.done { 'x' } else { ' ' };
                format!("- [{mark}] {}\n", subtask.text.trim())
            })
            .collect();
        if checklist.is_empty() {
            let description = self.description().trim_end();
            return if description.is_empty() {
                String::new()
            } else {
                format!("{description}\n")
            };
        }
        let head = self.head.trim_end();
        if head.is_empty() {
            checklist
        } else if self.checklist.is_empty() {
            format!("{head}\n\n{checklist}")
        } else {
            // Keep the gap the file already had between the two.
            format!("{}{checklist}", self.head)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Body, Subtask};

    fn sub(text: &str, done: bool) -> Subtask {
        Subtask {
            text: text.to_owned(),
            done,
        }
    }

    #[test]
    fn the_last_run_of_checklist_lines_holds_the_subtasks() {
        let body = Body::parse("Call first.\n\n- [ ] Buy seeds\n- [x] Rake\n");
        assert_eq!(body.description(), "Call first.");
        assert_eq!(body.subtasks, [sub("Buy seeds", false), sub("Rake", true)]);
    }

    #[test]
    fn a_checklist_in_the_middle_stays_in_the_description() {
        let text = "- [ ] One\n\nThen notes.\n";
        let body = Body::parse(text);
        assert_eq!(body.description(), text.trim_end());
        assert!(body.subtasks.is_empty());
    }

    #[test]
    fn a_body_without_a_checklist_is_all_description() {
        let body = Body::parse("Notes.\n");
        assert_eq!(body.description(), "Notes.");
        assert!(body.subtasks.is_empty());
        assert_eq!(Body::parse("").description(), "");
    }

    #[test]
    fn other_lines_are_not_subtasks() {
        for line in [
            "- [y] Odd\n",
            "- [ ]No space\n",
            "-[ ] Tight\n",
            "1. [ ] Numbered\n",
        ] {
            assert!(Body::parse(line).subtasks.is_empty(), "{line}");
        }
        assert_eq!(Body::parse("* [X] Star\n").subtasks, [sub("Star", true)]);
    }

    #[test]
    fn a_new_description_keeps_the_checklist_bytes() {
        let body = Body::parse("Old.\n\n- [X] Kept as written\n");
        assert_eq!(
            body.with_description("New notes."),
            "New notes.\n\n- [X] Kept as written\n"
        );
        assert_eq!(body.with_description(""), "- [X] Kept as written\n");
        assert_eq!(Body::parse("Old.\n").with_description("New."), "New.\n");
    }

    #[test]
    fn new_subtasks_keep_the_description_bytes() {
        let body = Body::parse("Notes  with   spaces.\r\n\r\n- [ ] One\n");
        assert_eq!(
            body.with_subtasks(&[sub("One", true), sub("Two", false)]),
            "Notes  with   spaces.\r\n\r\n- [x] One\n- [ ] Two\n"
        );
        assert_eq!(
            Body::parse("Notes.\n").with_subtasks(&[sub("First", false)]),
            "Notes.\n\n- [ ] First\n"
        );
        assert_eq!(
            Body::parse("").with_subtasks(&[sub("First", false)]),
            "- [ ] First\n"
        );
        assert_eq!(
            Body::parse("Notes.\n\n- [ ] Gone\n").with_subtasks(&[]),
            "Notes.\n"
        );
    }

    #[test]
    fn edits_round_trip() {
        let written = Body::parse("Plan.\n").with_subtasks(&[sub("A", false)]);
        let body = Body::parse(&written);
        assert_eq!(body.description(), "Plan.");
        assert_eq!(body.subtasks, [sub("A", false)]);
        assert_eq!(
            Body::parse(&body.with_description("Plan.")).subtasks,
            [sub("A", false)]
        );
    }
}
