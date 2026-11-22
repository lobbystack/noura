//! A typed name as a file name, by the workspace format's rules for
//! names, which `docs/workspace-format/v1.md` sets out under Names. The
//! Projects plugin keeps the same rules; the tests check both against
//! `workspace-format`'s own function, so none can drift apart.

/// The longest file name, in characters, that [`file_name`] keeps.
const MAX_FILE_NAME_CHARS: usize = 200;

/// Turn a typed name into a file name that works on macOS, Windows and
/// Linux: no path separators, no characters macOS or Windows reject, no
/// leading dots, no surrounding spaces, no trailing dots, at most 200
/// characters, and an underscore after a name Windows keeps for a device.
/// Empty when nothing is left.
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

/// The path that renames the file at `path` to `name`, in the same
/// folder.
pub(crate) fn renamed(path: &str, name: &str) -> String {
    match path.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}.md"),
        None => format!("{name}.md"),
    }
}

#[cfg(test)]
mod tests {
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
            assert_eq!(
                file_name(typed),
                workspace_format::normalize_file_name(typed),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn a_rename_keeps_the_folder() {
        assert_eq!(renamed("Calendars/Work.md", "Home"), "Calendars/Home.md");
        assert_eq!(renamed("Work.md", "Home"), "Home.md");
    }
}
