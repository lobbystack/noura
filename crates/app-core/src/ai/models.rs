//! The models an Orb can ask, and how hard each reasons: the efforts a
//! model takes, the one it uses when none is chosen, and what a request
//! sends the provider for an effort.
//!
//! The efforts follow each provider's documentation as of November 2026.
//! Anthropic: Claude 4.6 and later take `output_config.effort` with
//! adaptive thinking, Claude 5 and Fable think by default and some reject
//! `thinking: disabled`, older Claude 4 models take a thinking budget.
//! OpenAI: GPT-5.1 and later take `reasoning_effort` `none` to `high`,
//! GPT-5 `minimal` to `high`, and the o-series `low` to `high`. Gemini 2.5
//! takes a thinking budget and Gemini 3 and later a thinking level, and
//! Grok 3 Mini takes `low` or `high`. `genai` writes each provider's own
//! parameter from the [`AiReasoning`] a request carries.

use local_core::AiReasoning;
use plugin_host::{AiEffort, AiModel, AiProvider};

/// Anthropic's current models, which Noura lists for an Anthropic
/// provider beside its own model.
const ANTHROPIC_MODELS: [&str; 4] = [
    "claude-fable-5-2",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

const OFF_TO_HIGH: &[AiEffort] = &[
    AiEffort::Off,
    AiEffort::Low,
    AiEffort::Medium,
    AiEffort::High,
];
const LOW_TO_HIGH: &[AiEffort] = &[AiEffort::Low, AiEffort::Medium, AiEffort::High];
const LOW_OR_HIGH: &[AiEffort] = &[AiEffort::Low, AiEffort::High];

/// The efforts a model takes, from least to most, and the one it reasons
/// with when none is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Efforts {
    pub(crate) efforts: &'static [AiEffort],
    pub(crate) default: Option<AiEffort>,
    /// How Off reaches the provider: no reasoning, or the least it allows.
    off: AiReasoning,
    /// The model reasons only when a request asks it to. A request that
    /// answers its tool calls then asks for no reasoning: Noura keeps an
    /// answer's reasoning blocks only in memory, so a run resumed after a
    /// quit would send its tool results without them.
    asked: bool,
}

const fn efforts(efforts: &'static [AiEffort], default: Option<AiEffort>) -> Efforts {
    Efforts {
        efforts,
        default,
        off: AiReasoning::Zero,
        asked: false,
    }
}

/// The models of `providers`, provider by provider, each one's own model
/// first and then the others Noura knows for its kind.
pub(crate) fn models(providers: &[AiProvider]) -> Vec<AiModel> {
    let mut models: Vec<AiModel> = Vec::new();
    for provider in providers {
        let known: &[&str] = match provider.kind.as_str() {
            "anthropic" => &ANTHROPIC_MODELS,
            _ => &[],
        };
        let mut ids = vec![provider.model.as_str()];
        ids.extend(known.iter().copied().filter(|id| *id != provider.model));
        for id in ids {
            let efforts = efforts_of(&provider.kind, id);
            models.push(AiModel {
                provider: provider.id.clone(),
                id: id.to_owned(),
                name: name(&provider.kind, id),
                efforts: efforts
                    .map(|known| known.efforts.to_vec())
                    .unwrap_or_default(),
                default_effort: efforts.and_then(|known| known.default),
            });
        }
    }
    models
}

/// The name Noura shows for the model `id` of a provider of `kind`, such
/// as `Claude Opus 5.5`, or the ID itself.
pub(crate) fn name(kind: &str, id: &str) -> String {
    if kind != "anthropic" {
        return id.to_owned();
    }
    let Some(claude) = Claude::parse(id) else {
        return id.to_owned();
    };
    let family = match claude.family {
        Family::Opus => "Opus",
        Family::Sonnet => "Sonnet",
        Family::Haiku => "Haiku",
        Family::Fable => "Fable",
        Family::Mythos => "Mythos",
    };
    match claude.version {
        (major, 0) if !claude.minor => format!("Claude {family} {major}"),
        (major, minor) => format!("Claude {family} {major}.{minor}"),
    }
}

/// What the request asks the provider for when the model `model` of a
/// provider of `kind` reasons with `effort`, or none when the model takes
/// no such setting. `answers_tools` says the request answers the model's
/// tool calls.
pub(crate) fn reasoning(
    kind: &str,
    model: &str,
    effort: AiEffort,
    answers_tools: bool,
) -> Option<AiReasoning> {
    let known = efforts_of(kind, model)?;
    if !known.efforts.contains(&effort) || (known.asked && answers_tools) {
        return None;
    }
    Some(match effort {
        AiEffort::Off => known.off,
        AiEffort::Low => AiReasoning::Low,
        AiEffort::Medium => AiReasoning::Medium,
        AiEffort::High => AiReasoning::High,
    })
}

/// The efforts the model `model` of a provider of `kind` takes, or none
/// when Noura can't set how hard it reasons.
pub(crate) fn efforts_of(kind: &str, model: &str) -> Option<Efforts> {
    match kind {
        "anthropic" => anthropic(model),
        "openai" | "openai_resp" => openai(model, kind == "openai_resp"),
        "gemini" => gemini(model),
        "xai" if model.starts_with("grok-3-mini") => {
            Some(efforts(LOW_OR_HIGH, Some(AiEffort::Low)))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Opus,
    Sonnet,
    Haiku,
    Fable,
    Mythos,
}

/// A Claude model's family and version, as its ID names them.
struct Claude {
    family: Family,
    version: (u32, u32),
    /// The ID names a minor version, as `claude-opus-5-5` does and
    /// `claude-opus-5` doesn't.
    minor: bool,
}

impl Claude {
    /// `claude-<family>-<major>[-<minor>][-<date>]`, or the older
    /// `claude-<major>-<minor>-<family>[-<date>]`.
    fn parse(id: &str) -> Option<Self> {
        let parts: Vec<&str> = id.strip_prefix("claude-")?.split('-').collect();
        let family = |part: &str| match part {
            "opus" => Some(Family::Opus),
            "sonnet" => Some(Family::Sonnet),
            "haiku" => Some(Family::Haiku),
            "fable" => Some(Family::Fable),
            "mythos" => Some(Family::Mythos),
            _ => None,
        };
        // A version part has one or two digits; a date has eight.
        let number = |part: Option<&&str>| {
            part.filter(|part| (1..=2).contains(&part.len()))
                .and_then(|part| part.parse::<u32>().ok())
        };
        if let Some(family) = parts.first().and_then(|part| family(part)) {
            let major = number(parts.get(1))?;
            let minor = number(parts.get(2));
            return Some(Self {
                family,
                version: (major, minor.unwrap_or(0)),
                minor: minor.is_some(),
            });
        }
        let major = number(parts.first())?;
        let minor = number(parts.get(1));
        let at = if minor.is_some() { 2 } else { 1 };
        let family = parts.get(at).and_then(|part| family(part))?;
        Some(Self {
            family,
            version: (major, minor.unwrap_or(0)),
            minor: minor.is_some(),
        })
    }
}

/// Claude 5 and Fable reason by default and take an effort; Opus 5.5 and
/// Sonnet 5.5 refuse `thinking: disabled`, so they offer no Off. Claude
/// 4.6 to 4.8 reason only when asked, with an effort and adaptive
/// thinking; Opus 4.5 takes an effort without thinking; older Claude 4
/// and Claude 3.7 Sonnet take a thinking budget.
fn anthropic(model: &str) -> Option<Efforts> {
    let claude = Claude::parse(model)?;
    let asked = |efforts: Efforts| Efforts {
        asked: true,
        ..efforts
    };
    let high = Some(AiEffort::High);
    let medium = Some(AiEffort::Medium);
    let off = Some(AiEffort::Off);
    Some(match (claude.family, claude.version) {
        (Family::Fable | Family::Mythos, _) => efforts(LOW_TO_HIGH, high),
        (Family::Opus, version) if version >= (5, 5) => efforts(LOW_TO_HIGH, medium),
        (Family::Opus, (5, _)) => efforts(LOW_TO_HIGH, high),
        (Family::Sonnet, version) if version >= (5, 5) => efforts(LOW_TO_HIGH, high),
        (Family::Sonnet, (5, _)) => efforts(OFF_TO_HIGH, high),
        (Family::Haiku, (major, _)) if major >= 5 => efforts(OFF_TO_HIGH, medium),
        (Family::Opus, (4, 5)) => efforts(LOW_TO_HIGH, high),
        (Family::Opus | Family::Sonnet, (4, _)) | (Family::Haiku, (4, 5)) => {
            asked(efforts(OFF_TO_HIGH, off))
        }
        (Family::Sonnet, (3, 7)) => asked(efforts(OFF_TO_HIGH, off)),
        _ => return None,
    })
}

/// GPT-5.1 and later reason with `none` by default; GPT-5 turns down to
/// `minimal` at least. The Responses interface takes no `none` through
/// `genai`, so it offers no Off for them. The o-series take `low` to
/// `high`.
fn openai(model: &str, responses: bool) -> Option<Efforts> {
    if model.contains("-chat") {
        return None;
    }
    let medium = Some(AiEffort::Medium);
    if let Some(rest) = model.strip_prefix("gpt-5") {
        let versioned = rest.starts_with('.');
        return Some(if versioned && model.contains("codex") {
            efforts(LOW_TO_HIGH, medium)
        } else if versioned && responses {
            efforts(LOW_TO_HIGH, Some(AiEffort::Low))
        } else if versioned {
            efforts(OFF_TO_HIGH, Some(AiEffort::Off))
        } else {
            Efforts {
                off: AiReasoning::Minimal,
                ..efforts(OFF_TO_HIGH, medium)
            }
        });
    }
    if model.starts_with("gpt-6") {
        return Some(if responses {
            efforts(LOW_TO_HIGH, Some(AiEffort::Low))
        } else {
            efforts(OFF_TO_HIGH, Some(AiEffort::Off))
        });
    }
    let reasoning = ["o1", "o3", "o3-mini", "o3-pro", "o4-mini"]
        .iter()
        .any(|name| model == *name || model.strip_prefix(name).is_some_and(dated));
    reasoning.then(|| efforts(LOW_TO_HIGH, medium))
}

/// Whether `rest` is a date after a model's name, as in `o3-2025-04-16`.
fn dated(rest: &str) -> bool {
    rest.strip_prefix('-').is_some_and(|date| {
        !date.is_empty()
            && date
                .chars()
                .all(|letter| letter.is_ascii_digit() || letter == '-')
    })
}

/// Gemini 2.5 takes a thinking budget, which `genai` sets from low,
/// medium or high; Gemini 3 and later take a thinking level, of which
/// every model takes low and high.
fn gemini(model: &str) -> Option<Efforts> {
    let version = model.strip_prefix("gemini-")?;
    let major: u32 = version.split(['.', '-']).next()?.parse().ok()?;
    if major >= 3 {
        return Some(efforts(LOW_OR_HIGH, Some(AiEffort::High)));
    }
    version
        .starts_with("2.5")
        .then(|| efforts(LOW_TO_HIGH, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(kind: &str, model: &str) -> (Vec<AiEffort>, Option<AiEffort>) {
        efforts_of(kind, model).map_or((Vec::new(), None), |known| {
            (known.efforts.to_vec(), known.default)
        })
    }

    #[test]
    fn claude_models_take_the_efforts_their_generation_allows() {
        use AiEffort::{High, Low, Medium, Off};
        assert_eq!(
            offered("anthropic", "claude-opus-5-5"),
            (vec![Low, Medium, High], Some(Medium))
        );
        assert_eq!(
            offered("anthropic", "claude-sonnet-5-5"),
            (vec![Low, Medium, High], Some(High))
        );
        assert_eq!(
            offered("anthropic", "claude-haiku-5-5"),
            (vec![Off, Low, Medium, High], Some(Medium))
        );
        assert_eq!(
            offered("anthropic", "claude-fable-5-2"),
            (vec![Low, Medium, High], Some(High))
        );
        assert_eq!(
            offered("anthropic", "claude-opus-4-8"),
            (vec![Off, Low, Medium, High], Some(Off))
        );
        assert_eq!(
            offered("anthropic", "claude-sonnet-4-5-20250929"),
            (vec![Off, Low, Medium, High], Some(Off))
        );
        assert_eq!(
            offered("anthropic", "claude-3-7-sonnet-20250219"),
            (vec![Off, Low, Medium, High], Some(Off))
        );
        assert_eq!(offered("anthropic", "claude-3-5-haiku-20241022").0, []);
        assert_eq!(offered("anthropic", "my-proxy-model").0, []);
    }

    #[test]
    fn openai_reasoning_models_take_reasoning_effort() {
        use AiEffort::{High, Low, Medium, Off};
        assert_eq!(
            offered("openai", "gpt-5.1"),
            (vec![Off, Low, Medium, High], Some(Off))
        );
        assert_eq!(
            offered("openai", "gpt-5-mini"),
            (vec![Off, Low, Medium, High], Some(Medium))
        );
        assert_eq!(
            offered("openai", "o4-mini"),
            (vec![Low, Medium, High], Some(Medium))
        );
        assert_eq!(
            offered("openai", "o3-2025-04-16"),
            (vec![Low, Medium, High], Some(Medium))
        );
        assert_eq!(offered("openai", "gpt-4.1-mini").0, []);
        assert_eq!(offered("openai", "gpt-5.1-chat-latest").0, []);
        assert_eq!(offered("openai", "o1-mini").0, []);
        assert_eq!(offered("openai_resp", "gpt-5.1").0, [Low, Medium, High]);
    }

    #[test]
    fn off_reaches_each_provider_in_its_own_terms() {
        assert_eq!(
            reasoning("openai", "gpt-5.1", AiEffort::Off, false),
            Some(AiReasoning::Zero)
        );
        assert_eq!(
            reasoning("openai", "gpt-5", AiEffort::Off, false),
            Some(AiReasoning::Minimal)
        );
        assert_eq!(
            reasoning("anthropic", "claude-haiku-5-5", AiEffort::Off, false),
            Some(AiReasoning::Zero)
        );
        // Opus 5.5 can't turn reasoning off.
        assert_eq!(
            reasoning("anthropic", "claude-opus-5-5", AiEffort::Off, false),
            None
        );
        assert_eq!(
            reasoning("xai", "grok-3-mini", AiEffort::Medium, false),
            None
        );
        assert_eq!(reasoning("ollama", "llama3", AiEffort::High, false), None);
    }

    #[test]
    fn a_model_that_reasons_only_when_asked_answers_its_tool_calls_without() {
        assert_eq!(
            reasoning("anthropic", "claude-opus-4-8", AiEffort::High, false),
            Some(AiReasoning::High)
        );
        assert_eq!(
            reasoning("anthropic", "claude-opus-4-8", AiEffort::High, true),
            None
        );
        // Claude 5 reasons with each request anyway.
        assert_eq!(
            reasoning("anthropic", "claude-sonnet-5-5", AiEffort::Low, true),
            Some(AiReasoning::Low)
        );
    }

    #[test]
    fn gemini_takes_a_budget_before_3_and_a_level_after() {
        use AiEffort::{High, Low, Medium};
        assert_eq!(
            offered("gemini", "gemini-2.5-flash"),
            (vec![Low, Medium, High], None)
        );
        assert_eq!(
            offered("gemini", "gemini-3-pro"),
            (vec![Low, High], Some(High))
        );
        assert_eq!(offered("gemini", "gemini-2.0-flash").0, []);
    }

    #[test]
    fn claude_models_read_as_their_names() {
        assert_eq!(name("anthropic", "claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(name("anthropic", "claude-opus-5"), "Claude Opus 5");
        assert_eq!(
            name("anthropic", "claude-sonnet-4-5-20250929"),
            "Claude Sonnet 4.5"
        );
        assert_eq!(
            name("anthropic", "claude-3-7-sonnet-latest"),
            "Claude Sonnet 3.7"
        );
        assert_eq!(name("openai", "gpt-5.1"), "gpt-5.1");
    }

    #[test]
    fn each_provider_lists_its_own_model_first() {
        let provider = |id: &str, kind: &str, model: &str| AiProvider {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: kind.to_owned(),
            model: model.to_owned(),
        };
        let listed = models(&[
            provider("claude", "anthropic", "claude-sonnet-5-5"),
            provider("local", "ollama", "llama3"),
        ]);
        let ids: Vec<(&str, &str)> = listed
            .iter()
            .map(|model| (model.provider.as_str(), model.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("claude", "claude-sonnet-5-5"),
                ("claude", "claude-fable-5-2"),
                ("claude", "claude-opus-5-5"),
                ("claude", "claude-haiku-5-5"),
                ("local", "llama3"),
            ]
        );
        assert!(listed[4].efforts.is_empty());
        assert_eq!(listed[0].default_effort, Some(AiEffort::High));
    }
}
