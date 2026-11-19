//! The 1 MB note behind trial S1's typing measurement, and the generator
//! that wrote it.
//!
//! The note reads like a year of a team's working notes: meeting notes with
//! decisions and action items, project plans with milestone tables,
//! technical notes with code blocks and callouts, journal entries and
//! reading lists. A fixed pseudo-random sequence picks the words, so the
//! generator writes the same bytes on every platform and the checked-in
//! file can be compared against it.

use std::fmt::Write as _;

/// Where the checked-in note lives.
pub const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/large-note.md");

/// The generator stops after the first entry that takes the note past this
/// many bytes.
pub const TARGET_BYTES: usize = 1 << 20;

/// The sentence that opens the first entry.
pub const FIRST_SENTENCE: &str = "This notebook collects the team's working notes.";
/// The sentence of the entry written once the note passes half its size.
pub const MIDDLE_SENTENCE: &str = "Halfway through the year, the team reviewed every open plan.";
/// The sentence of the last entry.
pub const LAST_SENTENCE: &str = "These are the last notes of the year.";

/// SplitMix64: small, and the same on every platform and Rust release.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn between(&mut self, low: usize, high: usize) -> usize {
        low + self.below(high - low + 1)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const PROJECTS: &[&str] = &[
    "Atlas", "Harbor", "Lumen", "Juniper", "Mistral", "Quarry", "Saffron", "Tundra",
];
const PEOPLE: &[&str] = &[
    "Amira", "Ben", "Chloé", "Dmitri", "Elena", "Félix", "Grace", "Hiro", "Inès", "Jonas",
];
const TOPICS: &[&str] = &[
    "onboarding flow",
    "search index",
    "sync engine",
    "release checklist",
    "billing page",
    "offline mode",
    "keyboard shortcuts",
    "export to PDF",
    "calendar view",
    "plugin API",
    "error reporting",
    "settings screen",
];
const THINGS: &[&str] = &[
    "loading spinner",
    "empty state",
    "retry button",
    "date picker",
    "sidebar",
    "status badge",
    "progress bar",
    "conflict dialog",
];
const PROBLEMS: &[&str] = &[
    "flickers on slow networks",
    "takes two clicks to reach",
    "hides the real error",
    "drifts after a window resize",
    "times out under load",
    "reads poorly in dark mode",
];
const ACTIONS: &[&str] = &[
    "split the work into two tickets",
    "pair on it on Thursday",
    "add a regression test",
    "move it behind a setting",
    "write the migration notes",
    "ask legal for a review",
    "drop it from this release",
];
const MILESTONES: &[&str] = &[
    "beta",
    "public launch",
    "end of the quarter",
    "next release",
    "design review",
];
const REASONS: &[&str] = &[
    "the current version confuses new users",
    "support keeps getting the same question",
    "the old code path is hard to test",
    "we promised it to two customers",
    "the numbers from last month looked wrong",
];
const ADJECTIVES: &[&str] = &["confusing", "slow", "crowded", "too quiet", "easy to miss"];
const COMMANDS: &[&str] = &[
    "cargo test --workspace",
    "bun run check",
    "make lint",
    "./scripts/release.sh --dry-run",
];
const FRENCH: &[&str] = &[
    "On a convenu de garder la version actuelle jusqu'à la fin du mois.",
    "Félix a noté que l'écran des réglages était trop chargé.",
    "Inès propose de revoir la maquette avec l'équipe de soutien.",
];
const STATUSES: &[&str] = &["Done", "On track", "At risk", "Blocked", "Not started"];
const MONTHS: &[&str] = &[
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

/// A calendar day, advanced a few days per entry.
struct Day {
    month: usize,
    day: usize,
}

impl Day {
    fn advance(&mut self, days: usize) {
        self.day += days;
        if self.day > 28 {
            self.day -= 28;
            self.month = (self.month + 1) % 12;
        }
    }

    fn iso(&self) -> String {
        format!("2026-{:02}-{:02}", self.month + 1, self.day)
    }

    fn long(&self) -> String {
        format!("{} {}", MONTHS[self.month], self.day)
    }
}

fn slug(text: &str) -> String {
    text.to_lowercase().replace(' ', "-")
}

fn sentence(rng: &mut Rng) -> String {
    let person = rng.pick(PEOPLE);
    let other = rng.pick(PEOPLE);
    let topic = rng.pick(TOPICS);
    let thing = rng.pick(THINGS);
    let problem = rng.pick(PROBLEMS);
    let action = rng.pick(ACTIONS);
    match rng.below(14) {
        0 => format!(
            "{person} walked through the {topic} and pointed out that the {thing} still {problem}."
        ),
        1 => format!(
            "We agreed to {action} before the {}, because {}.",
            rng.pick(MILESTONES),
            rng.pick(REASONS)
        ),
        2 => format!(
            "The {topic} now loads in {} ms, about {}% faster than last month.",
            rng.between(80, 900),
            rng.between(5, 60)
        ),
        3 => format!(
            "{person} suggested we {action}; nobody objected, but {other} asked for a written plan first."
        ),
        4 => format!(
            "If the {thing} keeps misbehaving, we will {action} and revisit it at the next review."
        ),
        5 => format!(
            "Feedback on the {topic} was mostly positive, although a few people found the {thing} {}.",
            rng.pick(ADJECTIVES)
        ),
        6 => format!(
            "See [the design doc](https://docs.example.com/{}) and [[{} roadmap]] for the details.",
            slug(topic),
            rng.pick(PROJECTS)
        ),
        7 => format!(
            "The numbers come from the dashboard at https://metrics.example.com/{}.",
            slug(topic)
        ),
        8 => format!(
            "Remember to run `{}` before pushing, otherwise the {thing} {problem}.",
            rng.pick(COMMANDS)
        ),
        9 => "It is **not** a blocker yet, but it will be one *if we wait another sprint*."
            .to_string(),
        10 => rng.pick(FRENCH).to_string(),
        11 => {
            format!("{person} shipped the fix for the {thing} 🎉 and wrote it up in [[{topic}]].")
        }
        12 => format!(
            "The {topic} work for **{}** depends on the {thing}, which {problem}.",
            rng.pick(PROJECTS)
        ),
        _ => format!(
            "{other} will ==double-check== the {topic} numbers and report back on {}.",
            rng.pick(MILESTONES)
        ),
    }
}

/// A paragraph of `low` to `high` sentences on one line.
fn paragraph(rng: &mut Rng, low: usize, high: usize) -> String {
    let count = rng.between(low, high);
    (0..count)
        .map(|_| sentence(rng))
        .collect::<Vec<_>>()
        .join(" ")
}

fn meeting(rng: &mut Rng, out: &mut String, day: &Day) {
    let topic = rng.pick(TOPICS);
    let _ = writeln!(out, "## Meeting: {topic} ({})\n", day.iso());
    // Steps of 3 through 10 names never repeat one.
    let first = rng.below(PEOPLE.len());
    let attendees: Vec<&str> = (0..rng.between(3, 5))
        .map(|index| PEOPLE[(first + 3 * index) % PEOPLE.len()])
        .collect();
    let _ = writeln!(out, "Attendees: {}\n", attendees.join(", "));
    let _ = writeln!(out, "{}\n", paragraph(rng, 3, 7));
    out.push_str("### Decisions\n\n");
    for _ in 0..rng.between(2, 4) {
        let _ = writeln!(out, "- {}", sentence(rng));
        if rng.chance(50) {
            for _ in 0..rng.between(1, 3) {
                let _ = writeln!(out, "  - {}", sentence(rng));
                if rng.chance(25) {
                    let _ = writeln!(out, "    - {}", sentence(rng));
                }
            }
        }
    }
    out.push_str("\n### Action items\n\n");
    for _ in 0..rng.between(2, 5) {
        let mark = if rng.chance(40) { 'x' } else { ' ' };
        let _ = writeln!(
            out,
            "- [{mark}] {} to {} by {}",
            rng.pick(PEOPLE),
            rng.pick(ACTIONS),
            day.long()
        );
        if rng.chance(30) {
            let _ = writeln!(out, "  - [ ] {}", sentence(rng));
        }
    }
    out.push('\n');
}

fn plan(rng: &mut Rng, out: &mut String, day: &Day) {
    let project = rng.pick(PROJECTS);
    let _ = writeln!(out, "## {project} plan\n");
    let _ = writeln!(out, "{}\n", paragraph(rng, 6, 12));
    out.push_str("| Milestone | Owner | Due | Status |\n|:---|:---|---:|:---:|\n");
    for _ in 0..rng.between(3, 7) {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            rng.pick(MILESTONES),
            rng.pick(PEOPLE),
            day.iso(),
            rng.pick(STATUSES)
        );
    }
    out.push_str("\n### Steps\n\n");
    for step in 1..=rng.between(3, 6) {
        let _ = writeln!(out, "{step}. {}", sentence(rng));
        if rng.chance(40) {
            let _ = writeln!(out, "   - {}", sentence(rng));
        }
    }
    out.push('\n');
}

const CODE: &[(&str, &str)] = &[
    (
        "rust",
        "fn retry<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {\n    for delay in [100, 400, 1600] {\n        match attempt() {\n            Ok(value) => return Ok(value),\n            Err(err) if err.is_transient() => sleep(Duration::from_millis(delay)),\n            Err(err) => return Err(err),\n        }\n    }\n    attempt()\n}",
    ),
    (
        "ts",
        "export async function loadSettings(path: string): Promise<Settings> {\n\tconst text = await readFile(path, 'utf8');\n\tconst parsed = JSON.parse(text) as Partial<Settings>;\n\treturn { ...DEFAULT_SETTINGS, ...parsed };\n}",
    ),
    (
        "sh",
        "set -euo pipefail\nfor file in notes/*.md; do\n  wc -c \"$file\"\ndone | sort -n | tail -5",
    ),
    (
        "json",
        "{\n  \"name\": \"export\",\n  \"version\": 3,\n  \"targets\": [\"pdf\", \"html\"],\n  \"retry\": { \"attempts\": 3, \"backoff_ms\": 250 }\n}",
    ),
    (
        "sql",
        "SELECT project, count(*) AS open_tasks\nFROM tasks\nWHERE status <> 'done'\nGROUP BY project\nORDER BY open_tasks DESC;",
    ),
];

fn technical(rng: &mut Rng, out: &mut String) {
    let topic = rng.pick(TOPICS);
    let _ = writeln!(out, "## Notes on the {topic}\n");
    let _ = writeln!(out, "{}\n", paragraph(rng, 4, 9));
    let (language, code) = CODE[rng.below(CODE.len())];
    let _ = writeln!(out, "```{language}\n{code}\n```\n");
    if rng.chance(50) {
        let _ = writeln!(out, "> [!note] {}\n> {}\n", rng.pick(PEOPLE), sentence(rng));
    } else {
        let _ = writeln!(out, "> {}\n>\n> {}\n", sentence(rng), sentence(rng));
    }
    let _ = writeln!(out, "{}\n", paragraph(rng, 2, 5));
}

fn journal(rng: &mut Rng, out: &mut String, day: &Day) {
    let _ = writeln!(out, "## {}\n", day.long());
    let _ = writeln!(out, "{}\n", paragraph(rng, 8, 14));
    let _ = writeln!(out, "{}\n", paragraph(rng, 5, 10));
}

fn reading(rng: &mut Rng, out: &mut String) {
    out.push_str("## Reading list\n\n");
    for _ in 0..rng.between(3, 6) {
        let topic = rng.pick(TOPICS);
        match rng.below(3) {
            0 => {
                let _ = writeln!(
                    out,
                    "- [Notes on the {topic}](https://blog.example.com/{}): {}",
                    slug(topic),
                    sentence(rng)
                );
            }
            1 => {
                let _ = writeln!(out, "- https://www.example.org/articles/{}", slug(topic));
            }
            _ => {
                let _ = writeln!(out, "- [[{topic}]], from {}", rng.pick(PEOPLE));
            }
        }
    }
    out.push('\n');
}

/// Writes the note.
pub fn generate() -> String {
    let mut rng = Rng(0x5EED_0003);
    let mut day = Day { month: 0, day: 5 };
    let mut out = String::with_capacity(TARGET_BYTES + 16 * 1024);
    out.push_str(
        "---\ntags: [team, notebook]\naliases: [Work log 2026]\ncreated: 2026-01-05\n---\n\n",
    );
    out.push_str("# Team notebook 2026\n\n");
    let _ = writeln!(out, "{FIRST_SENTENCE} {}\n", paragraph(&mut rng, 3, 5));

    let mut wrote_middle = false;
    let mut entry = 0;
    while out.len() < TARGET_BYTES {
        if entry > 0 && entry % 40 == 0 {
            let _ = writeln!(out, "---\n\n# {}\n", MONTHS[day.month]);
        }
        if !wrote_middle && out.len() >= TARGET_BYTES / 2 {
            let _ = writeln!(
                out,
                "## Midyear review\n\n{MIDDLE_SENTENCE} {}\n",
                paragraph(&mut rng, 3, 5)
            );
            wrote_middle = true;
        }
        match rng.below(10) {
            0..=2 => meeting(&mut rng, &mut out, &day),
            3 | 4 => plan(&mut rng, &mut out, &day),
            5 | 6 => technical(&mut rng, &mut out),
            7 | 8 => journal(&mut rng, &mut out, &day),
            _ => reading(&mut rng, &mut out),
        }
        day.advance(rng.between(1, 3));
        entry += 1;
    }
    let _ = write!(
        out,
        "## Year-end notes\n\n{LAST_SENTENCE} {}\n",
        paragraph(&mut rng, 2, 4)
    );
    out
}
