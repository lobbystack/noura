//! What went wrong with a provider request, as a stable code.
//!
//! A provider answers a failed request with a status, a JSON body and a
//! request ID, or the request never reaches it. [`ProviderFailure`] keeps
//! only what decides the code: the status, the provider's own error type
//! and code, and whether its message speaks of credit or a model. The
//! request ID and those few words go to the local diagnostic log; the
//! provider's message, headers and the key never leave this module.

use super::*;

/// The provider refused the key, or the key can't do this.
pub const PROVIDER_AUTH_FAILED: &str = "provider_auth_failed";
/// The provider has no key in the credential store.
pub const PROVIDER_KEY_MISSING: &str = "provider_key_missing";
/// The account is out of credit or at its spending limit.
pub const PROVIDER_BILLING: &str = "provider_billing";
/// Too many requests for now.
pub const PROVIDER_RATE_LIMITED: &str = "provider_rate_limited";
/// The provider doesn't offer the model, or no longer does.
pub const PROVIDER_MODEL_NOT_FOUND: &str = "provider_model_not_found";
/// The chat is longer than the model reads.
pub const PROVIDER_TOO_LONG: &str = "provider_too_long";
/// The provider is down, overloaded or timed out.
pub const PROVIDER_OVERLOADED: &str = "provider_overloaded";
/// The request didn't reach the provider.
pub const PROVIDER_UNREACHABLE: &str = "provider_unreachable";
/// The connection broke while the answer streamed in.
pub const PROVIDER_CONNECTION_LOST: &str = "provider_connection_lost";
/// The provider turned the request down for another reason.
pub const PROVIDER_REJECTED: &str = "provider_rejected";
/// The ChatGPT plan's usage for this app ran out.
pub const PROVIDER_PLAN_LIMIT: &str = "provider_plan_limit";
/// Anything else.
pub const PROVIDER_REQUEST_FAILED: &str = "provider_request_failed";

/// The parts of a failed provider request that decide its code. Nothing
/// here is shown to anyone: the code is.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    /// The HTTP status, when the provider answered.
    pub status: Option<u16>,
    /// The provider's error type, such as `rate_limit_error`.
    pub error_type: Option<String>,
    /// The provider's finer code, such as `insufficient_quota`.
    pub error_code: Option<String>,
    /// The provider's request ID, for its support.
    pub request_id: Option<String>,
    /// The provider's message speaks of credit, billing or a balance.
    pub mentions_credit: bool,
    /// The provider's message names a missing model.
    pub mentions_model: bool,
    /// The provider's message says the input is too long.
    pub mentions_length: bool,
    /// The request never reached the provider.
    pub unreachable: bool,
    /// The connection broke after the answer began.
    pub connection_lost: bool,
}

impl ProviderFailure {
    /// The failure an HTTP answer with `status` and `body` describes, with
    /// the request ID from `request_id` when the body has none.
    pub fn from_http(status: u16, body: &str, request_id: Option<&str>) -> Self {
        let mut failure = Self::from_body(&serde_json::from_str(body).unwrap_or(Value::Null));
        failure.status = Some(status);
        if failure.request_id.is_none() {
            failure.request_id = request_id.and_then(id_token);
        }
        failure
    }

    /// The failure an error body describes: Anthropic's
    /// `{"type":"error","error":{..},"request_id":..}`, the OpenAI-style
    /// `{"error":{..}}`, Gemini's `{"error":{"status":..}}`, an error event
    /// inside a stream, or the error object itself.
    pub fn from_body(body: &Value) -> Self {
        let error = body
            .get("error")
            .filter(|error| error.is_object())
            .unwrap_or(body);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let error_type = error
            .get("type")
            .or_else(|| error.get("status"))
            .and_then(Value::as_str)
            .and_then(kind_token);
        // Gemini names the cause in `details[].reason`, as `API_KEY_INVALID`.
        let reason = error
            .get("details")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|detail| detail.get("reason").and_then(Value::as_str));
        let error_code = error
            .pointer("/details/error_code")
            .and_then(Value::as_str)
            .or(reason)
            .or_else(|| error.get("code").and_then(Value::as_str))
            .and_then(kind_token);
        let request_id = body
            .get("request_id")
            .and_then(Value::as_str)
            .and_then(id_token);
        Self {
            error_type,
            error_code,
            request_id,
            mentions_credit: ["credit", "billing", "balance", "quota", "payment"]
                .iter()
                .any(|word| message.contains(word)),
            mentions_model: message.contains("model")
                && [
                    "not found",
                    "does not exist",
                    "not exist",
                    "unknown",
                    "retired",
                ]
                .iter()
                .any(|words| message.contains(words)),
            mentions_length: [
                "too long",
                "context length",
                "maximum context",
                "too many tokens",
            ]
            .iter()
            .any(|words| message.contains(words)),
            ..Self::default()
        }
    }

    /// The stable code for this failure.
    pub fn code(&self) -> &'static str {
        if self.unreachable {
            return PROVIDER_UNREACHABLE;
        }
        if self.connection_lost {
            return PROVIDER_CONNECTION_LOST;
        }
        let named = [self.error_code.as_deref(), self.error_type.as_deref()];
        if let Some(code) = named.into_iter().flatten().find_map(code_of_kind) {
            return code;
        }
        if self.mentions_credit && self.status.is_none_or(|status| status < 500) {
            return PROVIDER_BILLING;
        }
        match self.status {
            Some(401 | 403) => PROVIDER_AUTH_FAILED,
            Some(402) => PROVIDER_BILLING,
            Some(404) => PROVIDER_MODEL_NOT_FOUND,
            Some(413) => PROVIDER_TOO_LONG,
            Some(429) => PROVIDER_RATE_LIMITED,
            Some(408 | 500..=599) => PROVIDER_OVERLOADED,
            Some(400..=499) if self.mentions_model => PROVIDER_MODEL_NOT_FOUND,
            Some(400..=499) if self.mentions_length => PROVIDER_TOO_LONG,
            Some(400..=499) => PROVIDER_REJECTED,
            _ if self.mentions_model => PROVIDER_MODEL_NOT_FOUND,
            _ => PROVIDER_REQUEST_FAILED,
        }
    }

    /// The public error for this failure, after the diagnostic log has
    /// what the provider said about it.
    pub fn into_error(self, provider_kind: &str, operation: &str) -> CoreError {
        let code = self.code();
        tracing::warn!(
            provider = provider_kind,
            code,
            status = self.status,
            error_type = self.error_type.as_deref(),
            error_code = self.error_code.as_deref(),
            request_id = self.request_id.as_deref(),
            "an AI provider request failed"
        );
        failure_error(code, operation)
    }
}

/// The public error for `code`.
pub(super) fn failure_error(code: &str, operation: &str) -> CoreError {
    let (category, message) = match code {
        PROVIDER_AUTH_FAILED => (ErrorCategory::Credential, "The provider refused the key"),
        PROVIDER_KEY_MISSING => (ErrorCategory::Credential, "The provider has no key"),
        PROVIDER_BILLING => (
            ErrorCategory::Provider,
            "The provider account can't pay for this",
        ),
        PROVIDER_RATE_LIMITED => (ErrorCategory::Transient, "The provider limited requests"),
        PROVIDER_MODEL_NOT_FOUND => (
            ErrorCategory::Provider,
            "The provider doesn't offer the model",
        ),
        PROVIDER_TOO_LONG => (
            ErrorCategory::Provider,
            "The chat is too long for the model",
        ),
        PROVIDER_OVERLOADED => (ErrorCategory::Transient, "The provider is unavailable"),
        PROVIDER_UNREACHABLE => (ErrorCategory::Transient, "The provider couldn't be reached"),
        PROVIDER_CONNECTION_LOST => (
            ErrorCategory::Transient,
            "The connection to the provider broke",
        ),
        PROVIDER_REJECTED => (ErrorCategory::Provider, "The provider rejected the request"),
        PROVIDER_PLAN_LIMIT => (ErrorCategory::Provider, "The plan's usage ran out"),
        _ => (ErrorCategory::Provider, "The AI provider request failed"),
    };
    let mut error = CoreError::new(code, category, message, operation);
    error.retryable = matches!(
        code,
        PROVIDER_RATE_LIMITED
            | PROVIDER_OVERLOADED
            | PROVIDER_UNREACHABLE
            | PROVIDER_CONNECTION_LOST
    );
    error
}

/// The code a provider's own error type or code names, when it names one
/// Noura knows.
fn code_of_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "authentication_error"
        | "permission_error"
        | "invalid_api_key"
        | "unauthenticated"
        | "permission_denied"
        | "api_key_invalid"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context"
        | "subscription_sharing_invalid_user" => PROVIDER_AUTH_FAILED,
        "missing_key" => PROVIDER_KEY_MISSING,
        "billing_error"
        | "insufficient_quota"
        | "enforced_spend_limit_reached"
        | "billing_not_active"
        | "billing_hard_limit_reached" => PROVIDER_BILLING,
        "rate_limit_error" | "rate_limit_exceeded" | "resource_exhausted" | "rate_limited" => {
            PROVIDER_RATE_LIMITED
        }
        "not_found_error" | "model_not_found" | "not_found" => PROVIDER_MODEL_NOT_FOUND,
        "request_too_large" | "context_length_exceeded" | "string_above_max_length" => {
            PROVIDER_TOO_LONG
        }
        "overloaded_error"
        | "api_error"
        | "timeout_error"
        | "server_error"
        | "unavailable"
        | "internal"
        | "deadline_exceeded"
        | "service_unavailable"
        | "subscription_sharing_usage_unavailable"
        | "subscription_sharing_user_unavailable" => PROVIDER_OVERLOADED,
        "subscription_sharing_usage_limit_exceeded"
        | "subscription_sharing_workplace_individual_usage_limit_exceeded"
        | "subscription_sharing_user_not_eligible" => PROVIDER_PLAN_LIMIT,
        _ => return None,
    })
}

/// `value`, lowercased, when it reads as an error type or code such as
/// `rate_limit_error`: letters, digits, underscores and dots. Anything
/// else could carry the provider's words, so it's dropped.
fn kind_token(value: &str) -> Option<String> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'));
    valid.then(|| value.to_ascii_lowercase())
}

/// `value` when it reads as a request ID, such as `req_011ChSPQ`.
fn id_token(value: &str) -> Option<String> {
    let valid = !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    valid.then(|| value.to_owned())
}

/// The failure behind a `genai` error, as far as it can tell.
pub(super) fn of_genai(error: &genai::Error) -> ProviderFailure {
    use genai::Error;
    match error {
        Error::HttpError {
            status,
            body,
            headers,
            ..
        } => ProviderFailure::from_http(status.as_u16(), body, request_id(headers)),
        Error::WebModelCall { webc_error, .. } | Error::WebAdapterCall { webc_error, .. } => {
            of_webc(webc_error)
        }
        Error::WebStream { error, .. } => of_boxed(error.as_ref()),
        Error::ChatResponse { body, .. } => ProviderFailure::from_body(body),
        Error::RequiresApiKey { .. } | Error::NoAuthData { .. } => ProviderFailure {
            error_type: Some("missing_key".into()),
            status: Some(401),
            ..ProviderFailure::default()
        },
        _ => ProviderFailure::default(),
    }
}

fn of_webc(error: &genai::webc::Error) -> ProviderFailure {
    match error {
        genai::webc::Error::ResponseFailedStatus {
            status,
            body,
            headers,
        } => ProviderFailure::from_http(status.as_u16(), body, request_id(headers)),
        genai::webc::Error::Reqwest(error) => of_reqwest(error),
        _ => ProviderFailure::default(),
    }
}

fn of_boxed(error: &(dyn std::error::Error + 'static)) -> ProviderFailure {
    if let Some(error) = error.downcast_ref::<genai::Error>() {
        return of_genai(error);
    }
    if let Some(error) = error.downcast_ref::<genai::webc::Error>() {
        return of_webc(error);
    }
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        return of_reqwest(error);
    }
    match error.source() {
        Some(source) => of_boxed(source),
        None => ProviderFailure::default(),
    }
}

pub(super) fn of_reqwest(error: &reqwest::Error) -> ProviderFailure {
    if let Some(status) = error.status() {
        return ProviderFailure {
            status: Some(status.as_u16()),
            ..ProviderFailure::default()
        };
    }
    if error.is_connect() || error.is_timeout() || error.is_request() {
        ProviderFailure {
            unreachable: true,
            ..ProviderFailure::default()
        }
    } else {
        ProviderFailure {
            connection_lost: error.is_body() || error.is_decode(),
            ..ProviderFailure::default()
        }
    }
}

fn request_id(headers: &reqwest::header::HeaderMap) -> Option<&str> {
    ["request-id", "x-request-id", "x-goog-request-id"]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(status: u16, body: &str) -> &'static str {
        ProviderFailure::from_http(status, body, None).code()
    }

    #[test]
    fn anthropic_out_of_credit_is_billing_though_it_comes_as_a_400() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."},"request_id":"req_011ChSPQ"}"#;
        let failure = ProviderFailure::from_http(400, body, None);
        assert_eq!(failure.code(), PROVIDER_BILLING);
        assert_eq!(failure.request_id.as_deref(), Some("req_011ChSPQ"));
        assert_eq!(
            code(
                402,
                r#"{"type":"error","error":{"type":"billing_error","message":"x"}}"#
            ),
            PROVIDER_BILLING
        );
    }

    #[test]
    fn each_status_and_provider_type_has_its_code() {
        for (status, body, expected) in [
            (
                401,
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
                PROVIDER_AUTH_FAILED,
            ),
            (
                401,
                r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
                PROVIDER_AUTH_FAILED,
            ),
            (
                400,
                r#"{"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#,
                PROVIDER_AUTH_FAILED,
            ),
            (
                403,
                r#"{"error":{"code":403,"message":"denied","status":"PERMISSION_DENIED"}}"#,
                PROVIDER_AUTH_FAILED,
            ),
            (
                429,
                r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
                PROVIDER_BILLING,
            ),
            (
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
                PROVIDER_RATE_LIMITED,
            ),
            (
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"You have reached your API usage limits","details":{"error_code":"enforced_spend_limit_reached"}}}"#,
                PROVIDER_BILLING,
            ),
            (
                429,
                r#"{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED"}}"#,
                PROVIDER_RATE_LIMITED,
            ),
            (
                404,
                r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-sonnet-4-5"}}"#,
                PROVIDER_MODEL_NOT_FOUND,
            ),
            (
                404,
                r#"{"error":{"message":"The model `gpt-4.1-mini` does not exist","type":"invalid_request_error","code":"model_not_found"}}"#,
                PROVIDER_MODEL_NOT_FOUND,
            ),
            (
                400,
                r#"{"error":{"message":"The model `x` does not exist or you do not have access to it."}}"#,
                PROVIDER_MODEL_NOT_FOUND,
            ),
            (
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#,
                PROVIDER_TOO_LONG,
            ),
            (
                400,
                r#"{"error":{"message":"x","code":"context_length_exceeded"}}"#,
                PROVIDER_TOO_LONG,
            ),
            (413, "", PROVIDER_TOO_LONG),
            (
                529,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                PROVIDER_OVERLOADED,
            ),
            (500, "<html>oops</html>", PROVIDER_OVERLOADED),
            (503, "", PROVIDER_OVERLOADED),
            (
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: bad"}}"#,
                PROVIDER_REJECTED,
            ),
            (422, "not json", PROVIDER_REJECTED),
            (
                429,
                r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"x"}}"#,
                PROVIDER_PLAN_LIMIT,
            ),
        ] {
            assert_eq!(code(status, body), expected, "{status} {body}");
        }
    }

    #[test]
    fn an_error_event_inside_a_stream_has_its_code() {
        let anthropic = serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        assert_eq!(
            ProviderFailure::from_body(&anthropic).code(),
            PROVIDER_OVERLOADED
        );
        let openai = serde_json::json!({"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"});
        assert_eq!(
            ProviderFailure::from_body(&openai).code(),
            PROVIDER_RATE_LIMITED
        );
    }

    #[test]
    fn only_identifiers_reach_the_log() {
        let body = r#"{"error":{"type":"has spaces and sk-secret","code":"sk-1234567890","message":"x"},"request_id":"req 1"}"#;
        let failure = ProviderFailure::from_http(400, body, Some("req_ok"));
        assert_eq!(failure.error_type, None);
        assert_eq!(failure.error_code, None);
        assert_eq!(failure.request_id.as_deref(), Some("req_ok"));
    }

    #[test]
    fn retryable_codes_say_so() {
        for (code, retryable) in [
            (PROVIDER_RATE_LIMITED, true),
            (PROVIDER_OVERLOADED, true),
            (PROVIDER_UNREACHABLE, true),
            (PROVIDER_BILLING, false),
            (PROVIDER_AUTH_FAILED, false),
        ] {
            assert_eq!(
                failure_error(code, "ai_stream").retryable,
                retryable,
                "{code}"
            );
        }
    }
}
