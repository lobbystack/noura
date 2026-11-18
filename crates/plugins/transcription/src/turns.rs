//! `Transcript.md`'s body: one paragraph per speaker turn, starting with
//! who spoke and when, as `**Me** · 00:03:12`, then what they said. Back
//! from the file, each turn becomes a line of the Summary's prompt.

use plugin_sdk::speech::{Channel, Segment};

use crate::prompt::Line;

/// The name a channel's speaker goes by.
pub(crate) fn speaker(channel: Channel) -> &'static str {
    match channel {
        Channel::Me => "Me",
        Channel::Others => "Others",
    }
}

/// `ms` from the start of the recording, as `HH:MM:SS`.
pub(crate) fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// One speaker's turn: the segments in a row from one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Turn {
    pub(crate) channel: Channel,
    pub(crate) start_ms: u64,
    pub(crate) text: String,
}

/// The turns of `segments`, in time order: segments in a row from the
/// same channel join into one turn, and empty ones drop out.
pub(crate) fn turns(segments: &[Segment]) -> Vec<Turn> {
    let mut sorted: Vec<&Segment> = segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .collect();
    sorted.sort_by_key(|segment| segment.start_ms);
    let mut turns: Vec<Turn> = Vec::new();
    for segment in sorted {
        let text = one_line(&segment.text);
        match turns.last_mut() {
            Some(turn) if turn.channel == segment.channel => {
                turn.text.push(' ');
                turn.text.push_str(&text);
            }
            _ => turns.push(Turn {
                channel: segment.channel,
                start_ms: segment.start_ms,
                text,
            }),
        }
    }
    turns
}

/// What a model said, on one line, so it can't end the paragraph or start
/// a heading.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Transcript's Markdown: each turn's paragraph, a blank line between.
pub(crate) fn markdown(turns: &[Turn]) -> String {
    let mut out = String::new();
    for turn in turns {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "**{}** · {}\n{}\n",
            speaker(turn.channel),
            clock(turn.start_ms),
            turn.text
        ));
    }
    out
}

/// The lines of a Transcript's body, as you may have edited it: each
/// paragraph that starts with a speaker and a time is that speaker's
/// line, and any other paragraph stays as it is, without a speaker.
pub(crate) fn lines(body: &str) -> Vec<Line> {
    let body = body.replace("\r\n", "\n");
    body.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            let (first, rest) = paragraph.split_once('\n').unwrap_or((paragraph, ""));
            match header(first) {
                Some((speaker, after)) => Line {
                    speaker: speaker.to_owned(),
                    text: one_line(&format!("{after} {rest}")),
                },
                None => Line {
                    speaker: "Unknown".to_owned(),
                    text: one_line(paragraph),
                },
            }
        })
        .filter(|line| !line.text.is_empty())
        .collect()
}

/// The speaker of a turn's first line, `**Name** · 00:00:00`, and the
/// text after the time.
fn header(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("**")?;
    let (name, rest) = rest.split_once("**")?;
    let rest = rest.trim_start().strip_prefix('·')?.trim_start();
    let (time, after) = rest.split_at(rest.find(char::is_whitespace).unwrap_or(rest.len()));
    let parts: Vec<&str> = time.split(':').collect();
    let is_time = parts.len() == 3
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()));
    (is_time && !name.trim().is_empty()).then_some((name.trim(), after.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(channel: Channel, start_ms: u64, text: &str) -> Segment {
        Segment {
            channel,
            start_ms,
            end_ms: start_ms + 1000,
            text: text.into(),
        }
    }

    #[test]
    fn each_speaker_turn_is_one_paragraph_with_its_time() {
        let segments = [
            segment(Channel::Others, 195_000, "Sounds good."),
            segment(Channel::Me, 192_000, "Let's ship\n it."),
            segment(Channel::Me, 3_600_000 + 61_000, "Later."),
            segment(Channel::Others, 196_000, "  "),
            segment(Channel::Others, 197_000, "I'll write it up."),
        ];
        let turns = turns(&segments);
        assert_eq!(
            markdown(&turns),
            "**Me** · 00:03:12\nLet's ship it.\n\n\
             **Others** · 00:03:15\nSounds good. I'll write it up.\n\n\
             **Me** · 01:01:01\nLater.\n"
        );
        assert_eq!(markdown(&[]), "");
    }

    #[test]
    fn an_edited_transcript_reads_back_as_lines() {
        let body = "**Me** · 00:03:12\nLet's ship it.\n\n**Others** · 00:03:15 Sounds good.\r\n\r\n\
                    A note I added.\n\n**Bold** words · 12:00 aren't a turn.\n";
        assert_eq!(
            lines(body),
            [
                Line {
                    speaker: "Me".into(),
                    text: "Let's ship it.".into()
                },
                Line {
                    speaker: "Others".into(),
                    text: "Sounds good.".into()
                },
                Line {
                    speaker: "Unknown".into(),
                    text: "A note I added.".into()
                },
                Line {
                    speaker: "Unknown".into(),
                    text: "**Bold** words · 12:00 aren't a turn.".into()
                },
            ]
        );
        assert!(lines("\n\n").is_empty());
    }
}
