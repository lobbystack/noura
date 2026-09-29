//! Provider settings: validation, secure endpoints, credentials, and the
//! fingerprint that ties consent to one provider configuration.

use super::*;

pub(super) fn provider_fingerprint(provider: &AiProviderConfig) -> String {
    let mut fingerprint = blake3::Hasher::new();
    for value in [
        &provider.kind,
        provider.endpoint.as_deref().unwrap_or(""),
        &provider.model,
    ] {
        fingerprint.update(&(value.len() as u64).to_le_bytes());
        fingerprint.update(value.as_bytes());
    }
    fingerprint.finalize().to_hex().to_string()
}

pub(super) fn credential_for(
    provider: &AiProviderConfig,
    operation: &str,
) -> Result<Option<String>> {
    provider
        .credential_ref
        .as_ref()
        .map(|reference| {
            keyring::Entry::new("org.noura.ai", reference)
                .and_then(|entry| entry.get_password())
                .map_err(|_| credential_error(operation))
        })
        .transpose()
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
    if AdapterKind::from_lower_str(&provider.kind).is_none() {
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
            "Use an HTTPS address. Plain HTTP works only for a server on this computer.",
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
    CoreError::new(
        "credential_store_error",
        ErrorCategory::Credential,
        "The operating system credential store operation failed",
        operation,
    )
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
