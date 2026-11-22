//! A trigger in plain words, as the Orb's side panel lists it:
//! `event transcription.finished` reads as `When a meeting ends`, and
//! `schedule 30 8 * * 1-5` as `Weekdays at 8:30`. The Orb's file keeps
//! the trigger as it is; only its words on screen change.

/// The events Noura's own plugins send, in words.
const EVENTS: [(&str, &str); 1] = [("transcription.finished", "When a meeting ends")];

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `trigger` in plain words, or a readable form of it when Noura can't
/// phrase it.
pub(crate) fn words(trigger: &str) -> String {
    let trigger = trigger.trim();
    if let Some(event) = trigger.strip_prefix("event ") {
        return event_words(event.trim());
    }
    if let Some(pattern) = trigger.strip_prefix("schedule ") {
        let pattern = pattern.trim();
        return schedule_words(pattern).unwrap_or_else(|| format!("On a schedule: {pattern}"));
    }
    trigger.to_owned()
}

/// An event a plugin sends, such as `transcription.finished`.
fn event_words(event: &str) -> String {
    if let Some((_, said)) = EVENTS.iter().find(|(known, _)| *known == event) {
        return (*said).to_owned();
    }
    match event.split_once('.') {
        Some((plugin, name)) if !plugin.is_empty() && !name.is_empty() => {
            format!("When {}: {}", capitalized(plugin), spaced(name))
        }
        _ => format!("When {event}"),
    }
}

/// A 5-field cron pattern, or a nickname such as `@daily`, in words.
fn schedule_words(pattern: &str) -> Option<String> {
    let pattern = match pattern.to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *".to_owned(),
        "@daily" | "@midnight" => "0 0 * * *".to_owned(),
        "@weekly" => "0 0 * * 0".to_owned(),
        "@monthly" => "0 0 1 * *".to_owned(),
        "@yearly" | "@annually" => "0 0 1 1 *".to_owned(),
        other => other.to_owned(),
    };
    let fields: Vec<&str> = pattern.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields.as_slice() else {
        return None;
    };
    let every = |field: &str| field == "*" || field == "?";
    // Every few minutes or hours, whatever the day.
    if every(day) && every(month) && every(weekday) {
        if let Some(step) = minute.strip_prefix("*/").and_then(number)
            && every(hour)
        {
            return Some(match step {
                1 => "Every minute".to_owned(),
                step => format!("Every {step} minutes"),
            });
        }
        if every(hour) {
            let minute = number(minute)?;
            return Some(match minute {
                0 => "Every hour".to_owned(),
                minute => format!("Every hour at :{minute:02}"),
            });
        }
        if let Some(step) = hour.strip_prefix("*/").and_then(number) {
            let minute = number(minute)?;
            return Some(match (step, minute) {
                (1, 0) => "Every hour".to_owned(),
                (step, 0) => format!("Every {step} hours"),
                (step, minute) => format!("Every {step} hours at :{minute:02}"),
            });
        }
    }
    let time = time(minute, hour)?;
    if every(month) && every(weekday) {
        if every(day) {
            return Some(format!("Every day at {time}"));
        }
        let day = number(day).filter(|day| (1..=31).contains(day))?;
        return Some(format!("On the {} of each month at {time}", ordinal(day)));
    }
    if every(day) && every(month) {
        return Some(format!("{} at {time}", weekdays(weekday)?));
    }
    if every(weekday) {
        let day = number(day).filter(|day| (1..=31).contains(day))?;
        let month = month_name(month)?;
        return Some(format!("Every {month} {day} at {time}"));
    }
    None
}

/// The time a schedule fires each day it fires, such as `8:30`, when its
/// minute and hour are one value each.
fn time(minute: &str, hour: &str) -> Option<String> {
    let minute = number(minute).filter(|minute| *minute < 60)?;
    let hour = number(hour).filter(|hour| *hour < 24)?;
    Some(format!("{hour}:{minute:02}"))
}

/// The days of the week a schedule names, in words: `Weekdays`,
/// `Weekends`, `Every Monday` or `Every Monday and Thursday`.
fn weekdays(field: &str) -> Option<String> {
    let mut days = Vec::new();
    for part in field.split(',') {
        match part.split_once('-') {
            Some((first, last)) => {
                let (first, last) = (weekday(first)?, weekday(last)?);
                if first > last {
                    return None;
                }
                days.extend(first..=last);
            }
            None => days.push(weekday(part)?),
        }
    }
    days.sort_unstable();
    days.dedup();
    Some(match days.as_slice() {
        [1, 2, 3, 4, 5] => "Weekdays".to_owned(),
        [0, 6] => "Weekends".to_owned(),
        [0, 1, 2, 3, 4, 5, 6] => "Every day".to_owned(),
        days => {
            let names: Vec<&str> = days.iter().map(|day| DAYS[*day]).collect();
            format!("Every {}", listed(&names))
        }
    })
}

/// A day of the week as cron names it: `0` to `7`, where both `0` and
/// `7` are Sunday, or its first three letters.
fn weekday(text: &str) -> Option<usize> {
    if let Some(day) = number(text) {
        return (day <= 7).then_some(day % 7);
    }
    DAYS.iter()
        .position(|name| name[..3].eq_ignore_ascii_case(text))
}

/// A month as cron names it: `1` to `12`, or its first three letters.
fn month_name(text: &str) -> Option<&'static str> {
    if let Some(month) = number(text) {
        return (1..=12).contains(&month).then(|| MONTHS[month - 1]);
    }
    MONTHS
        .iter()
        .find(|name| name[..3].eq_ignore_ascii_case(text))
        .copied()
}

fn number(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `1st`, `2nd`, `3rd`, `11th`, `22nd` and so on.
fn ordinal(number: usize) -> String {
    let suffix = match (number % 10, number % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{number}{suffix}")
}

/// `a`, `a and b`, or `a, b and c`.
fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// An event's name with its separators as spaces: `task_done` reads as
/// `task done`.
fn spaced(name: &str) -> String {
    name.replace(['_', '-', '.'], " ")
}

#[cfg(test)]
mod tests {
    use super::words;

    #[test]
    fn the_meeting_event_reads_as_a_meeting_ending() {
        assert_eq!(words("event transcription.finished"), "When a meeting ends");
        assert_eq!(words("event tasks.task_done"), "When Tasks: task done");
    }

    #[test]
    fn common_schedules_read_as_plain_words() {
        for (trigger, said) in [
            ("schedule 30 8 * * 1-5", "Weekdays at 8:30"),
            ("schedule 30 8 * * MON-FRI", "Weekdays at 8:30"),
            ("schedule 0 9 * * 1", "Every Monday at 9:00"),
            ("schedule 0 9 * * mon", "Every Monday at 9:00"),
            ("schedule 0 18 * * *", "Every day at 18:00"),
            ("schedule 0 10 * * 0,6", "Weekends at 10:00"),
            ("schedule 0 10 * * 7", "Every Sunday at 10:00"),
            (
                "schedule 0 17 * * 1,3,5",
                "Every Monday, Wednesday and Friday at 17:00",
            ),
            (
                "schedule 15 7 * * 2,4",
                "Every Tuesday and Thursday at 7:15",
            ),
            ("schedule 0 9 1 * *", "On the 1st of each month at 9:00"),
            ("schedule 0 9 22 * *", "On the 22nd of each month at 9:00"),
            ("schedule 0 8 1 1 *", "Every January 1 at 8:00"),
            ("schedule */15 * * * *", "Every 15 minutes"),
            ("schedule 0 * * * *", "Every hour"),
            ("schedule 30 * * * *", "Every hour at :30"),
            ("schedule 0 */2 * * *", "Every 2 hours"),
            ("schedule @daily", "Every day at 0:00"),
            ("schedule @weekly", "Every Sunday at 0:00"),
            ("schedule @hourly", "Every hour"),
        ] {
            assert_eq!(words(trigger), said, "{trigger}");
        }
    }

    #[test]
    fn a_schedule_noura_cant_phrase_stays_readable() {
        assert_eq!(words("schedule 0 9 1-7 * 1"), "On a schedule: 0 9 1-7 * 1");
        assert_eq!(
            words("schedule 0,30 9-17 * * 1-5"),
            "On a schedule: 0,30 9-17 * * 1-5"
        );
        assert_eq!(words("whenever"), "whenever");
    }
}
