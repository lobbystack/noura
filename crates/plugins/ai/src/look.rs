//! How an Orb looks and reads: its avatar, the dot that says how it
//! stands, its efforts' names, and how long ago a run started.

use plugin_sdk::{
    Avatar, Color,
    ai::{self, Effort, Orb, OrbRunStatus, OrbState},
};

/// The default Orb's ID, which has no file.
pub(crate) const DEFAULT_ORB: &str = "orb_00000000000000000000000000";

/// The colors an Orb without one of its own takes, by its ID, so each
/// Orb keeps its own. Yellow and gray read poorly behind white text.
const COLORS: [Color; 6] = [
    Color::Blue,
    Color::Purple,
    Color::Green,
    Color::Orange,
    Color::Red,
    Color::Brown,
];

/// The default Orb as the page shows it when Noura can't list the Orbs.
pub(crate) fn default_orb() -> Orb {
    Orb {
        id: DEFAULT_ORB.to_owned(),
        title: "Noura".to_owned(),
        revision: String::new(),
        avatar: None,
        color: None,
        provider: None,
        model: None,
        effort: None,
        state: OrbState::Idle,
        thread: None,
        tools: Vec::new(),
        triggers: Vec::new(),
        problem: None,
    }
}

/// The palette color an Orb's file names.
fn color_named(name: &str) -> Option<Color> {
    Some(match name {
        "red" => Color::Red,
        "orange" => Color::Orange,
        "yellow" => Color::Yellow,
        "green" => Color::Green,
        "blue" => Color::Blue,
        "purple" => Color::Purple,
        "brown" => Color::Brown,
        "gray" => Color::Gray,
        _ => return None,
    })
}

/// The Orb's color: its own, or the one its ID picks.
pub(crate) fn color(orb: &Orb) -> Color {
    if let Some(color) = orb.color.as_deref().and_then(color_named) {
        return color;
    }
    if orb.id == DEFAULT_ORB {
        return Color::Blue;
    }
    let sum = orb.id.bytes().fold(0usize, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(usize::from(byte))
    });
    COLORS[sum % COLORS.len()]
}

/// The Orb's avatar: its color, with its own emoji or letters, or the
/// first letter of its name.
pub(crate) fn avatar(orb: &Orb) -> Avatar {
    let text = orb
        .avatar
        .clone()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| {
            orb.title
                .chars()
                .find(|letter| letter.is_alphanumeric())
                .map(|letter| letter.to_uppercase().collect())
        });
    Avatar {
        text,
        color: Some(color(orb)),
    }
}

/// The dot that says how the Orb stands, when it isn't just idle: its
/// words and color. `failed` says its last run failed.
pub(crate) fn status(orb: &Orb, failed: bool) -> Option<(&'static str, Color)> {
    match orb.state {
        OrbState::Working => Some(("Working", Color::Blue)),
        OrbState::Held => Some(("Waiting", Color::Orange)),
        OrbState::Paused => Some(("Paused", Color::Gray)),
        OrbState::Problem => Some(("Problem", Color::Red)),
        OrbState::Idle if failed => Some(("Failed", Color::Red)),
        OrbState::Idle => None,
    }
}

/// An effort's choice ID, as an Orb's file names it.
pub(crate) fn effort_id(effort: Effort) -> &'static str {
    match effort {
        Effort::Off => "off",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
    }
}

pub(crate) fn effort_label(effort: Effort) -> &'static str {
    match effort {
        Effort::Off => "Off",
        Effort::Low => "Low",
        Effort::Medium => "Medium",
        Effort::High => "High",
    }
}

pub(crate) fn effort_named(id: &str) -> Option<Effort> {
    [Effort::Off, Effort::Low, Effort::Medium, Effort::High]
        .into_iter()
        .find(|effort| effort_id(*effort) == id)
}

/// A run's status in a word.
pub(crate) fn run_status(status: OrbRunStatus, quiet: bool) -> &'static str {
    match status {
        OrbRunStatus::Running => "Running",
        OrbRunStatus::Succeeded if quiet => "Nothing new",
        OrbRunStatus::Succeeded => "Done",
        OrbRunStatus::Failed => "Failed",
        OrbRunStatus::Interrupted | OrbRunStatus::Cancelled => "Stopped",
    }
}

/// How long before `now` the instant `then` was, both in RFC 3339:
/// `Now`, `12 min ago`, `3 h ago` or `4 d ago`.
pub(crate) fn ago(then: &str, now: &str) -> Option<String> {
    let seconds = seconds(now)? - seconds(then)?;
    let minutes = seconds.max(0) / 60;
    Some(match minutes {
        0 => "Now".to_owned(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    })
}

/// Seconds since 1970 of an RFC 3339 instant, such as
/// `2026-11-21T12:00:00Z` or `2026-11-21T07:00:00.5-05:00`.
fn seconds(instant: &str) -> Option<i64> {
    let (date, time) = instant.split_once('T')?;
    let mut parts = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let clock = time.get(..8)?;
    let mut fields = clock.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        fields.next()?.ok()?,
        fields.next()?.ok()?,
        fields.next()?.ok()?,
    );
    let zone =
        time[8..].trim_start_matches(|letter: char| letter == '.' || letter.is_ascii_digit());
    let offset = match zone {
        "Z" | "z" | "" => 0,
        zone => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = zone.get(1..)?.split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };
    // Days from the civil date, after Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The models' choice ID for `model` of `provider`: both, as JSON, so
/// neither needs a separator it can't hold.
pub(crate) fn model_id(provider: &str, model: &str) -> String {
    serde_json::json!([provider, model]).to_string()
}

/// The provider and model a models' choice ID names.
pub(crate) fn model_named(id: &str) -> Option<(String, String)> {
    let [provider, model]: [String; 2] = serde_json::from_str(id).ok()?;
    Some((provider, model))
}

/// The provider an Orb asks: its own, when you still have it, or your
/// first.
pub(crate) fn provider_of(orb: &Orb, providers: &[ai::Provider]) -> Option<ai::Provider> {
    orb.provider
        .as_deref()
        .and_then(|chosen| providers.iter().find(|provider| provider.id == chosen))
        .or_else(|| providers.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_reads_as_how_long_ago_it_started() {
        let now = "2026-11-21T12:00:00Z";
        assert_eq!(ago("2026-11-21T11:59:30Z", now).as_deref(), Some("Now"));
        assert_eq!(
            ago("2026-11-21T11:48:00Z", now).as_deref(),
            Some("12 min ago")
        );
        assert_eq!(ago("2026-11-21T09:00:00Z", now).as_deref(), Some("3 h ago"));
        assert_eq!(ago("2026-11-17T12:00:00Z", now).as_deref(), Some("4 d ago"));
        assert_eq!(
            ago("2026-11-21T06:30:00.250-05:00", now).as_deref(),
            Some("30 min ago")
        );
        assert_eq!(ago("yesterday", now), None);
    }

    #[test]
    fn a_model_choice_names_its_provider_and_model() {
        let id = model_id("my \"claude\"", "claude-opus-5-5");
        assert_eq!(
            model_named(&id),
            Some(("my \"claude\"".to_owned(), "claude-opus-5-5".to_owned()))
        );
        assert_eq!(model_named("claude-opus-5-5"), None);
    }
}
