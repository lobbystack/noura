//! Where a new project's file goes: a folder of its own under `projects/`,
//! named like the file, as `projects/Garden/Garden.md`, so the folder can
//! hold the project's notes and files. The kernel names an object's file
//! after its title in a folder named after its type, so Projects picks the
//! path itself and gives it to the host.
//!
//! A typed title becomes a file name by the workspace format's rules for
//! names, which `docs/workspace-format/v1.md` sets out under Names. The
//! tests check these against `workspace-format`'s own function, so the two
//! can't drift apart.

/// The longest file name, in characters, that [`file_name`] keeps, as the
/// workspace format sets it.
const MAX_FILE_NAME_CHARS: usize = 200;

/// The most names [`candidates`] gives: the title, then the title with
/// each number from 1 up.
const MAX_CANDIDATES: u32 = 10_000;

/// Turn a typed title into a file name that works on macOS, Windows and
/// Linux, as the workspace format does: no path separators, no characters
/// macOS or Windows reject, no leading dots, no surrounding spaces, no
/// trailing dots, at most 200 characters, and an underscore after a name
/// Windows keeps for a device. Empty when nothing is left.
pub(crate) fn file_name(typed: &str) -> String {
    let kept: String = typed
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        .collect();
    let dot_or_space = |character: char| character == '.' || character.is_whitespace();
    let name = kept.trim_start_matches(dot_or_space).trim_end();
    let cut: String = name.chars().take(MAX_FILE_NAME_CHARS).collect();
    let name = cut.trim_end_matches(dot_or_space);
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if is_windows_device_name(stem) {
        let (stem, rest) = name.split_at(stem.len());
        return format!("{stem}_{rest}");
    }
    name.to_owned()
}

/// Whether Windows keeps `name` for a device: `CON`, `PRN`, `AUX`, `NUL`,
/// or `COM` or `LPT` followed by a digit from 1 to 9, in any letter case.
fn is_windows_device_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            upper.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && matches!(upper.as_bytes()[3], b'1'..=b'9')
        }
    }
}

/// The names to try for a new project titled `title`, in order: its file
/// name, or `Untitled` when nothing is left of it, then that name with 1,
/// 2 and so on after it, cut so each stays within 200 characters.
pub(crate) fn candidates(title: &str) -> impl Iterator<Item = String> {
    let base = file_name(title);
    let base = if base.is_empty() {
        "Untitled".to_owned()
    } else {
        base
    };
    (0..MAX_CANDIDATES).map(move |number| {
        let suffix = match number {
            0 => String::new(),
            _ => format!(" {number}"),
        };
        let room = MAX_FILE_NAME_CHARS - suffix.chars().count();
        let stem: String = base.chars().take(room).collect();
        format!("{}{suffix}", stem.trim_end())
    })
}

/// The path of the project named `name`: its file in a folder of its own.
pub(crate) fn project_path(name: &str) -> String {
    format!("projects/{name}/{name}.md")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::Value;

    use super::*;

    #[test]
    fn typed_names_become_file_names_as_the_format_fixture_says() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        for case in fixture["file_names"].as_array().unwrap() {
            let typed = case["typed"].as_str().unwrap();
            assert_eq!(
                file_name(typed),
                case["name"].as_str().unwrap(),
                "{typed:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn file_names_match_the_workspace_formats(typed in any::<String>()) {
            prop_assert_eq!(file_name(&typed), workspace_format::normalize_file_name(&typed));
        }

        #[test]
        fn file_names_of_dotted_and_device_names_match_the_workspace_formats(
            typed in "[ .]{0,3}(con|PRN|aux|Nul|com[0-9]|LPT[0-9]|plan)[ .:/]{0,3}[a-z.]{0,4}",
        ) {
            prop_assert_eq!(file_name(&typed), workspace_format::normalize_file_name(&typed));
        }
    }

    #[test]
    fn a_project_gets_a_folder_named_like_its_file() {
        let mut names = candidates(" Garden: v2 ");
        assert_eq!(names.next().unwrap(), "Garden v2");
        assert_eq!(names.next().unwrap(), "Garden v2 1");
        assert_eq!(names.next().unwrap(), "Garden v2 2");
        assert_eq!(project_path("Garden v2"), "projects/Garden v2/Garden v2.md");
        assert_eq!(candidates("///").next().unwrap(), "Untitled");
    }

    #[test]
    fn a_numbered_name_stays_within_the_longest_file_name() {
        let long = "a".repeat(250);
        let names: Vec<String> = candidates(&long).take(12).collect();
        assert_eq!(names[0], "a".repeat(200));
        assert_eq!(names[1], format!("{} 1", "a".repeat(198)));
        assert_eq!(names[11], format!("{} 11", "a".repeat(197)));
        assert!(names.iter().all(|name| name.chars().count() <= 200));
        assert_eq!(candidates("x").count(), 10_000);
    }
}
