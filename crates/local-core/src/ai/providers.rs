//! Provider settings: validation, secure endpoints and credentials.

use super::*;

/// The provider kinds Noura offers, each with the name Settings shows, in
/// the order it lists them. Each is one that `genai` connects to.
pub const AI_PROVIDER_KINDS: [(&str, &str); 12] = [
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
    ("gemini", "Google Gemini"),
    ("ollama", "Ollama"),
    ("groq", "Groq"),
    ("deepseek", "DeepSeek"),
    ("xai", "xAI"),
    ("together", "Together AI"),
    ("fireworks", "Fireworks AI"),
    ("cohere", "Cohere"),
    ("open_router", "OpenRouter"),
    ("openai_resp", "OpenAI Responses"),
];

/// Models their providers stopped serving, or will on the date given,
/// each with the one Noura moves a saved provider to: `(kind, model,
/// replacement, last day served)`. Checked against each provider's own
/// deprecation pages on 2026-11-27:
///
/// - Anthropic: https://platform.claude.com/docs/en/about-claude/model-deprecations
/// - OpenAI: https://developers.openai.com/api/docs/deprecations
/// - Gemini: https://ai.google.dev/gemini-api/docs/deprecations
/// - Groq: https://console.groq.com/docs/deprecations
/// - DeepSeek: https://api-docs.deepseek.com/updates
/// - xAI: https://docs.x.ai/docs/models, which no longer lists Grok 3 Mini
/// - Together AI: https://docs.together.ai/docs/deprecations
/// - Fireworks AI: https://fireworks.ai/models/fireworks/llama-v3p3-70b-instruct,
///   which is no longer served without a deployment of your own
pub const RETIRED_MODELS: [(&str, &str, &str, &str); 10] = [
    (
        "anthropic",
        "claude-sonnet-4-5",
        "claude-sonnet-5-5",
        "2026-11-30",
    ),
    (
        "anthropic",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-5-5",
        "2026-11-30",
    ),
    ("openai", "gpt-4.1-mini", "gpt-6-luna", "2027-01-14"),
    (
        "openai",
        "gpt-4.1-mini-2025-04-14",
        "gpt-6-luna",
        "2027-01-14",
    ),
    (
        "gemini",
        "gemini-2.5-flash",
        "gemini-4-hydrogen",
        "2026-12-01",
    ),
    (
        "groq",
        "llama-3.3-70b-versatile",
        "openai/gpt-oss-120b",
        "2026-08-16",
    ),
    ("deepseek", "deepseek-chat", "deepseek-flash", "2026-07-24"),
    ("xai", "grok-3-mini", "grok-4.3", "2026-05-15"),
    (
        "together",
        "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        "openai/gpt-oss-120b",
        "2026-10-22",
    ),
    (
        "fireworks",
        "accounts/fireworks/models/llama-v3p3-70b-instruct",
        "accounts/fireworks/models/deepseek-v4p1-flash",
        "2026-11-27",
    ),
];

/// The model Noura moves `model` of a provider of `kind` to once `today`
/// is past the last day its provider served it, or `None` while it's
/// still served or isn't retired.
pub fn replacement_model(
    kind: &str,
    model: &str,
    today: jiff::civil::Date,
) -> Option<&'static str> {
    RETIRED_MODELS
        .iter()
        .find(|(retired_kind, retired, _, _)| *retired_kind == kind && *retired == model)
        .filter(|(_, _, _, last)| {
            last.parse::<jiff::civil::Date>()
                .is_ok_and(|last| today > last)
        })
        .map(|(_, _, replacement, _)| *replacement)
}

/// Move each hosted provider in `providers` whose model its provider no
/// longer serves to the replacement, and say whether any moved. A
/// provider at an address you set keeps its model: that server decides
/// what it serves.
pub(super) fn retire_models(providers: &mut [AiProviderConfig], today: jiff::civil::Date) -> bool {
    let mut moved = false;
    for provider in providers
        .iter_mut()
        .filter(|provider| provider.endpoint.is_none())
    {
        if let Some(replacement) = replacement_model(&provider.kind, &provider.model, today) {
            tracing::info!(
                provider = provider.kind.as_str(),
                retired = provider.model.as_str(),
                replacement,
                "an AI provider's model was retired, so it moved to the replacement"
            );
            provider.model = replacement.to_owned();
            moved = true;
        }
    }
    moved
}

/// A model a request names in place of its provider's own: as a
/// provider's model must be.
pub(super) fn validate_model(model: &str, operation: &str) -> Result<()> {
    if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "model_invalid",
            "The model name is not valid",
            operation,
        ));
    }
    Ok(())
}

pub(super) fn validate_provider(provider: &AiProviderConfig, operation: &str) -> Result<()> {
    if provider.id.is_empty()
        || provider.id.len() > 128
        || provider.id.chars().any(char::is_control)
        || provider.model.trim().is_empty()
        || provider.model.len() > 256
        || provider.model.chars().any(char::is_control)
    {
        return Err(CoreError::validation(
            "provider_invalid",
            "Provider ID and model are required",
            operation,
        ));
    }
    if provider.kind != chatgpt::CHATGPT_KIND
        && AdapterKind::from_lower_str(&provider.kind).is_none()
    {
        return Err(CoreError::validation(
            "provider_kind_invalid",
            "The AI provider kind is not supported",
            operation,
        ));
    }
    if let Some(endpoint) = &provider.endpoint {
        let url = url::Url::parse(endpoint).map_err(|_| {
            CoreError::validation(
                "endpoint_invalid",
                "The provider endpoint must be a valid URL",
                operation,
            )
        })?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(CoreError::validation(
                "endpoint_invalid",
                "Provider endpoints use HTTP or HTTPS",
                operation,
            ));
        }
    }
    Ok(())
}

/// Plain HTTP would send the API key and workspace content in the clear, so
/// it is only allowed for a model server on this computer (Ollama, LM
/// Studio). Saving and streaming check this; loading does not, so settings
/// saved by an older version stay readable and fixable.
pub(super) fn ensure_secure_endpoint(provider: &AiProviderConfig, operation: &str) -> Result<()> {
    let Some(endpoint) = &provider.endpoint else {
        return Ok(());
    };
    let url = url::Url::parse(endpoint).map_err(|_| {
        CoreError::validation(
            "endpoint_invalid",
            "The provider endpoint must be a valid URL",
            operation,
        )
    })?;
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    let secure = match url.scheme() {
        "https" => url.host().is_some(),
        "http" => loopback,
        _ => false,
    };
    if secure {
        Ok(())
    } else {
        Err(CoreError::validation(
            "endpoint_insecure",
            "Use HTTPS, or HTTP only for a server on this computer.",
            operation,
        ))
    }
}

pub(super) fn validate_provider_configs(
    providers: &[AiProviderConfig],
    operation: &str,
) -> Result<()> {
    let mut ids = std::collections::HashSet::with_capacity(providers.len());
    for provider in providers {
        validate_provider(provider, operation)?;
        if !ids.insert(&provider.id) {
            return Err(CoreError::new(
                "provider_config_invalid",
                ErrorCategory::Parse,
                "AI provider settings contain duplicate provider IDs",
                operation,
            ));
        }
    }
    Ok(())
}

pub(super) fn provider_lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "provider_lock_unavailable",
        ErrorCategory::Transient,
        "AI provider settings are unavailable",
        operation,
    )
}

pub(super) fn provider_error(operation: &str) -> CoreError {
    CoreError::new(
        "provider_request_failed",
        ErrorCategory::Provider,
        "The AI provider request failed",
        operation,
    )
}

pub(super) fn credential_error(operation: &str) -> CoreError {
    crate::credentials::credential_error(operation)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, "ai_provider_save", parent.to_str()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| {
        CoreError::new(
            "provider_serialize_failed",
            ErrorCategory::Parse,
            "AI provider settings could not be serialized",
            "ai_provider_save",
        )
    })?;
    crate::durable_settings::write(path, &bytes)
        .map_err(|error| CoreError::io(error, "ai_provider_save", path.to_str()))?;
    Ok(())
}

#[cfg(test)]
mod retirement_tests {
    use super::*;

    fn hosted(kind: &str, model: &str) -> AiProviderConfig {
        AiProviderConfig {
            id: kind.into(),
            kind: kind.into(),
            display_name: kind.into(),
            model: model.into(),
            endpoint: None,
            credential_ref: None,
            enabled: true,
            context_window: None,
        }
    }

    #[test]
    fn a_saved_provider_moves_off_its_model_after_its_last_day_and_not_before() {
        let last: jiff::civil::Date = "2026-11-30".parse().unwrap();
        let mut providers = vec![hosted("anthropic", "claude-sonnet-4-5")];
        assert!(!retire_models(&mut providers, last));
        assert_eq!(providers[0].model, "claude-sonnet-4-5");
        assert!(retire_models(&mut providers, last.tomorrow().unwrap()));
        assert_eq!(providers[0].model, "claude-sonnet-5-5");
        assert!(!retire_models(&mut providers, last.tomorrow().unwrap()));
    }

    #[test]
    fn a_provider_at_your_own_address_keeps_its_model() {
        let mut providers = vec![AiProviderConfig {
            endpoint: Some("http://localhost:1234/v1/".into()),
            ..hosted("openai", "gpt-4.1-mini")
        }];
        assert!(!retire_models(
            &mut providers,
            "2030-01-01".parse().unwrap()
        ));
        assert_eq!(providers[0].model, "gpt-4.1-mini");
    }

    #[test]
    fn each_retired_model_has_a_date_and_a_replacement_that_isnt_retired() {
        for (kind, model, replacement, last) in RETIRED_MODELS {
            assert!(AdapterKind::from_lower_str(kind).is_some(), "{kind}");
            assert!(last.parse::<jiff::civil::Date>().is_ok(), "{model}: {last}");
            assert_ne!(model, replacement);
            assert!(
                replacement_model(kind, replacement, "2100-01-01".parse().unwrap()).is_none(),
                "{replacement} is retired too"
            );
        }
    }
}
