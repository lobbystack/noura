//! What a failed answer says: one sentence for the code Noura recorded,
//! naming the provider and what to do next. The provider's own words
//! never reach the plugin, only the code.

use plugin_sdk::ai;

/// Where you review and limit what Noura uses of your ChatGPT plan.
pub(crate) const MANAGE_USAGE: &str = "https://chatgpt.com/settings/usage";

/// The sentence for an answer that failed with `code`, from the provider
/// named `provider`, or `None` when the provider isn't known.
pub(crate) fn line(code: Option<&str>, provider: Option<&str>) -> String {
    let named = provider.filter(|name| !name.trim().is_empty());
    let start = named.unwrap_or("The provider");
    let within = named.unwrap_or("the provider");
    match code.unwrap_or_default() {
        "provider_auth_failed" => {
            format!("{start} didn't accept the key, so update it in AI settings.")
        }
        "provider_key_missing" => format!("{start} has no key yet, so add one in AI settings."),
        "provider_billing" => format!(
            "Your {within} account is out of credit or at its spending limit, so check its billing."
        ),
        "provider_rate_limited" => {
            format!("{start} is limiting requests, so wait a minute and try again.")
        }
        "provider_model_not_found" | "provider_model_retired" => {
            format!("{start} doesn't offer this model anymore, so pick another one for this Orb.")
        }
        "provider_too_long" => {
            "This chat is too long for the model, so start a new chat.".to_owned()
        }
        "provider_overloaded" => format!("{start} is down or busy, so try again in a few minutes."),
        "provider_unreachable" => {
            format!("Noura couldn't reach {within}, so check your internet connection.")
        }
        "provider_connection_lost" => {
            format!("The connection to {within} dropped, so try again.")
        }
        "provider_rejected" => {
            format!("{start} turned down this request, so try again or pick another model.")
        }
        "provider_plan_limit" => {
            "Noura used its share of your ChatGPT plan, so check Manage usage below.".to_owned()
        }
        "provider_signed_out" => {
            format!("Noura is signed out of {within}, so sign in again in AI settings.")
        }
        "provider_stream_incomplete" => {
            format!("{start} stopped before the answer finished, so try again.")
        }
        "provider_not_found" => {
            "This Orb's provider is gone or turned off, so choose another in AI settings."
                .to_owned()
        }
        "credential_store_error" => {
            "Noura couldn't read the key from the keychain, so add it again in AI settings."
                .to_owned()
        }
        code if code.starts_with("provider_") || code.is_empty() => {
            format!("{start} couldn't answer, so try again.")
        }
        _ => "Noura couldn't finish this answer, so try again.".to_owned(),
    }
}

/// Whether [`line`] says something particular about `code`, so the run's
/// own failure line would only repeat it.
pub(crate) fn speaks_for(code: &str) -> bool {
    code.starts_with("provider_") || code == "credential_store_error"
}

/// The name of the provider `id` among `providers`.
pub(crate) fn provider_name<'a>(
    id: Option<&str>,
    providers: &'a [ai::Provider],
) -> Option<&'a str> {
    let id = id?;
    providers
        .iter()
        .find(|provider| provider.id == id)
        .map(|provider| provider.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_code_names_the_provider_and_the_next_step_in_one_sentence() {
        for code in [
            "provider_auth_failed",
            "provider_key_missing",
            "provider_billing",
            "provider_rate_limited",
            "provider_model_not_found",
            "provider_overloaded",
            "provider_unreachable",
            "provider_connection_lost",
            "provider_rejected",
            "provider_stream_incomplete",
            "provider_request_failed",
        ] {
            let line = line(Some(code), Some("Anthropic"));
            assert!(line.contains("Anthropic"), "{code}: {line}");
            assert!(line.ends_with('.'), "{code}: {line}");
            assert_eq!(line.matches(". ").count(), 0, "{code}: {line}");
        }
        assert_eq!(
            line(Some("provider_billing"), Some("Anthropic")),
            "Your Anthropic account is out of credit or at its spending limit, so check its billing."
        );
    }

    #[test]
    fn an_unknown_provider_reads_as_the_provider() {
        assert_eq!(
            line(Some("provider_auth_failed"), None),
            "The provider didn't accept the key, so update it in AI settings."
        );
        assert_eq!(
            line(Some("provider_unreachable"), Some(" ")),
            "Noura couldn't reach the provider, so check your internet connection."
        );
    }

    #[test]
    fn a_code_of_noura_s_own_reads_as_noura_s() {
        assert_eq!(
            line(Some("filesystem_error"), Some("Anthropic")),
            "Noura couldn't finish this answer, so try again."
        );
        assert!(!speaks_for("filesystem_error"));
        assert!(speaks_for("provider_billing"));
    }
}
