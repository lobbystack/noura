//! Which files open in the editor as plain text: the legacy app's list of
//! text and code files, ported from `apps/app/src/lib/editor/text-files.ts`.

/// The extensions of the text and code files the editor opens, lowercase.
const TEXT_EXTENSIONS: [&str; 37] = [
    "txt", "json", "jsonc", "ts", "tsx", "js", "jsx", "mjs", "cjs", "svelte", "rs", "toml", "yaml",
    "yml", "css", "scss", "html", "htm", "xml", "csv", "sql", "py", "sh", "mdx", "c", "cpp", "h",
    "go", "java", "rb", "php", "swift", "kt", "ini", "conf", "log", "vue",
];

/// The names of text files that have no extension.
const TEXT_NAMES: [&str; 4] = ["makefile", "dockerfile", "license", "notice"];

/// Whether the file at the workspace path `relative` is a plain-text or code
/// file the editor opens as source, by its extension or its whole name, in
/// any letter case. Markdown files are notes, not on this list. Opening still
/// checks that the bytes are UTF-8.
pub fn is_plain_text_path(relative: &str) -> bool {
    let name = relative
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative)
        .to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((_, extension)) => TEXT_EXTENSIONS.contains(&extension),
        None => TEXT_NAMES.contains(&name.as_str()),
    }
}

/// Whether the file at the workspace path `relative` is Markdown.
pub(crate) fn is_markdown_text(relative: &str) -> bool {
    relative.to_ascii_lowercase().ends_with(".md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_legacy_list_of_text_files_opens_as_text() {
        for path in [
            "a.txt",
            "Docs/data.JSON",
            "Cargo.toml",
            "ci.yml",
            "config.yaml",
            "src/main.rs",
            "web/page.htm",
            "web/page.html",
            "script.py",
            "run.sh",
            "Makefile",
            "build/dockerfile",
            "LICENSE",
            "NOTICE",
            "notes.mdx",
            "server.log",
        ] {
            assert!(is_plain_text_path(path), "{path}");
        }
    }

    #[test]
    fn notes_binaries_and_unknown_names_stay_off_the_list() {
        for path in [
            "Plan.md",
            "photo.png",
            "doc.pdf",
            "archive.tar.gz",
            "README",
            "LICENSE.md",
            "Makefile.am",
            "folder.txt/file",
            "",
        ] {
            assert!(!is_plain_text_path(path), "{path}");
        }
    }
}
