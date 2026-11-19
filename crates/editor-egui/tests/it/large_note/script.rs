//! The keystroke script trial S1 replays on the 1 MB note, and its parser.
//! The file's header documents the format.

use egui::{Key, Modifiers};

/// Where the checked-in script lives.
pub const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/large-note.keys"
);

/// Where a `goto` step puts the caret.
#[derive(Clone, Debug, PartialEq)]
pub enum Goto {
    Start,
    End,
    Before(String),
    After(String),
}

impl Goto {
    /// The caret's byte offset in `text`, or `None` if the anchor text
    /// isn't there.
    pub fn offset(&self, text: &str) -> Option<usize> {
        match self {
            Goto::Start => Some(0),
            Goto::End => Some(text.len()),
            Goto::Before(anchor) => text.find(anchor.as_str()),
            Goto::After(anchor) => text.find(anchor.as_str()).map(|at| at + anchor.len()),
        }
    }
}

/// One line of the script.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Places the caret without a keystroke. The replay doesn't time it.
    Goto(Goto),
    /// Keystrokes, each delivered and timed on its own.
    Keys {
        /// What the keystrokes do, for the report: `type`, `Enter`,
        /// `Backspace`, `paste`, `select`.
        kind: String,
        events: Vec<egui::Event>,
    },
}

/// Replaces `\n`, `\t` and `\\` with what they stand for.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn key_press(key: Key, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Parses one `key` line's arguments: a key name, then `shift`, `command`
/// or `alt`, then an optional repeat count written `xN`.
fn parse_key(args: &str) -> Result<Step, String> {
    let mut words = args.split_whitespace();
    let name = words.next().ok_or("a key step needs a key")?;
    let key = Key::from_name(name).ok_or_else(|| format!("no key is named {name:?}"))?;
    let mut modifiers = Modifiers::NONE;
    let mut times = 1;
    for word in words {
        match word {
            "shift" => modifiers |= Modifiers::SHIFT,
            "command" => modifiers |= Modifiers::COMMAND,
            "alt" => modifiers |= Modifiers::ALT,
            _ => {
                times = word
                    .strip_prefix('x')
                    .and_then(|count| count.parse().ok())
                    .ok_or_else(|| format!("{word:?} is neither a modifier nor a count"))?;
            }
        }
    }
    let kind = if modifiers.shift && matches!(key, Key::ArrowUp | Key::ArrowDown) {
        "select".to_string()
    } else {
        name.to_string()
    };
    Ok(Step::Keys {
        kind,
        events: vec![key_press(key, modifiers); times],
    })
}

fn parse_line(line: &str) -> Result<Step, String> {
    let (command, args) = line.split_once(' ').unwrap_or((line, ""));
    match command {
        "goto" => Ok(Step::Goto(match args {
            "start" => Goto::Start,
            "end" => Goto::End,
            _ => match args.split_once(' ') {
                Some(("before", anchor)) => Goto::Before(anchor.to_string()),
                Some(("after", anchor)) => Goto::After(anchor.to_string()),
                _ => return Err(format!("can't go to {args:?}")),
            },
        })),
        "type" if !args.is_empty() => Ok(Step::Keys {
            kind: "type".into(),
            events: unescape(args)
                .chars()
                .map(|ch| egui::Event::Text(ch.to_string()))
                .collect(),
        }),
        "paste" if !args.is_empty() => Ok(Step::Keys {
            kind: "paste".into(),
            events: vec![egui::Event::Paste(unescape(args))],
        }),
        "key" => parse_key(args),
        _ => Err(format!("unknown step {line:?}")),
    }
}

/// The steps of a script. Blank lines and lines starting with `#` don't
/// count.
pub fn parse(script: &str) -> Result<Vec<Step>, String> {
    script
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|(index, line)| parse_line(line).map_err(|err| format!("line {}: {err}", index + 1)))
        .collect()
}

/// The checked-in script's steps.
pub fn checked_in() -> Vec<Step> {
    let script = std::fs::read_to_string(PATH).expect("the script is checked in");
    parse(&script).expect("the script parses")
}
