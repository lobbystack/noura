//! How each provider's failures reach you: as a stable code, whatever the
//! provider answered, with none of its words, headers or the key.

use std::sync::Arc;

use local_core::{AiFoundation, AiProviderConfig, AiStreamEvent, CoreError, MemoryCredentials};
use serde_json::json;
use tokio::net::TcpListener;

use super::ai_provider::{Answer, KEY, LocalProvider, events, settings, stream, text};

/// The error a stream ended with.
async fn failure(kind: &str, answer: Answer) -> CoreError {
    let provider = LocalProvider::start(answer).await;
    let settings = settings(kind, &provider.endpoint);
    let events = events(stream(&settings.ai, vec![text("user", "Hello.")])).await;
    provider.heard().await;
    match events.last() {
        Some(AiStreamEvent::Error { error }) => {
            let shown = serde_json::to_string(error).unwrap();
            for raw in [KEY, "secret-provider-words", "req_"] {
                assert!(!shown.contains(raw), "{raw} reached you: {shown}");
            }
            error.clone()
        }
        other => panic!("the stream didn't fail: {other:?}"),
    }
}

/// What each provider answers for each failure, and the code it reaches
/// you as.
fn cases() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        // Anthropic, https://platform.claude.com/docs/en/api/errors
        (
            "anthropic",
            "400 Bad Request",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. secret-provider-words"},"request_id":"req_011abc"}"#,
            "provider_billing",
        ),
        (
            "anthropic",
            "401 Unauthorized",
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key secret-provider-words"},"request_id":"req_011abc"}"#,
            "provider_auth_failed",
        ),
        (
            "anthropic",
            "402 Payment Required",
            r#"{"type":"error","error":{"type":"billing_error","message":"secret-provider-words"}}"#,
            "provider_billing",
        ),
        (
            "anthropic",
            "403 Forbidden",
            r#"{"type":"error","error":{"type":"permission_error","message":"secret-provider-words"}}"#,
            "provider_auth_failed",
        ),
        (
            "anthropic",
            "404 Not Found",
            r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-sonnet-4-5 secret-provider-words"}}"#,
            "provider_model_not_found",
        ),
        (
            "anthropic",
            "413 Payload Too Large",
            r#"{"type":"error","error":{"type":"request_too_large","message":"secret-provider-words"}}"#,
            "provider_too_long",
        ),
        (
            "anthropic",
            "429 Too Many Requests",
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"secret-provider-words"}}"#,
            "provider_rate_limited",
        ),
        (
            "anthropic",
            "500 Internal Server Error",
            r#"{"type":"error","error":{"type":"api_error","message":"secret-provider-words"}}"#,
            "provider_overloaded",
        ),
        (
            "anthropic",
            "529 Overloaded",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"secret-provider-words"}}"#,
            "provider_overloaded",
        ),
        // OpenAI and the providers that speak its format.
        (
            "openai",
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key secret-provider-words","type":"invalid_request_error","code":"invalid_api_key"}}"#,
            "provider_auth_failed",
        ),
        (
            "openai",
            "404 Not Found",
            r#"{"error":{"message":"The model `gpt-4.1-mini` does not exist secret-provider-words","type":"invalid_request_error","code":"model_not_found"}}"#,
            "provider_model_not_found",
        ),
        (
            "openai",
            "429 Too Many Requests",
            r#"{"error":{"message":"You exceeded your current quota secret-provider-words","type":"insufficient_quota","code":"insufficient_quota"}}"#,
            "provider_billing",
        ),
        (
            "openai",
            "429 Too Many Requests",
            r#"{"error":{"message":"Rate limit reached secret-provider-words","type":"requests","code":"rate_limit_exceeded"}}"#,
            "provider_rate_limited",
        ),
        (
            "openai",
            "400 Bad Request",
            r#"{"error":{"message":"secret-provider-words","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
            "provider_too_long",
        ),
        (
            "openai",
            "503 Service Unavailable",
            "upstream connect error secret-provider-words",
            "provider_overloaded",
        ),
        (
            "groq",
            "401 Unauthorized",
            r#"{"error":{"message":"Invalid API Key secret-provider-words","type":"invalid_request_error","code":"invalid_api_key"}}"#,
            "provider_auth_failed",
        ),
        (
            "openai_resp",
            "401 Unauthorized",
            r#"{"error":{"message":"secret-provider-words","type":"invalid_request_error","code":"invalid_api_key"}}"#,
            "provider_auth_failed",
        ),
        (
            "openai_resp",
            "429 Too Many Requests",
            r#"{"error":{"message":"secret-provider-words","code":"rate_limit_exceeded"}}"#,
            "provider_rate_limited",
        ),
        // Gemini
        (
            "gemini",
            "400 Bad Request",
            r#"{"error":{"code":400,"message":"API key not valid. secret-provider-words","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}"#,
            "provider_auth_failed",
        ),
        (
            "gemini",
            "404 Not Found",
            r#"{"error":{"code":404,"message":"models/gemini-x is not found secret-provider-words","status":"NOT_FOUND"}}"#,
            "provider_model_not_found",
        ),
        (
            "gemini",
            "429 Too Many Requests",
            r#"{"error":{"code":429,"message":"secret-provider-words","status":"RESOURCE_EXHAUSTED"}}"#,
            "provider_rate_limited",
        ),
        (
            "gemini",
            "503 Service Unavailable",
            r#"{"error":{"code":503,"message":"secret-provider-words","status":"UNAVAILABLE"}}"#,
            "provider_overloaded",
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn each_provider_failure_reaches_you_as_its_code() {
    for (kind, status, body, code) in cases() {
        let error = failure(kind, Answer::Status(status, body)).await;
        assert_eq!(error.code, code, "{kind} {status} {body}");
        assert_eq!(
            error.retryable,
            matches!(code, "provider_rate_limited" | "provider_overloaded"),
            "{kind} {status}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_event_inside_an_anthropic_stream_has_its_code() {
    let answer = vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "model": "test-model",
                "content": [], "stop_reason": null,
                "usage": { "input_tokens": 3, "output_tokens": 0 },
            },
        }),
        json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "Overloaded secret-provider-words" },
        }),
    ];
    let error = failure("anthropic", Answer::Events(answer)).await;
    assert_eq!(error.code, "provider_overloaded");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_event_inside_an_openai_stream_has_its_code() {
    let answer = vec![json!({
        "error": {
            "message": "Rate limit reached secret-provider-words",
            "type": "requests",
            "code": "rate_limit_exceeded",
        },
    })];
    let error = failure("openai", Answer::Events(answer)).await;
    assert_eq!(error.code, "provider_rate_limited");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_nobody_answers_for_is_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
    drop(listener);
    for kind in ["openai", "anthropic", "gemini"] {
        let settings = settings(kind, &endpoint);
        let events = events(stream(&settings.ai, vec![text("user", "Hello.")])).await;
        let Some(AiStreamEvent::Error { error }) = events.last() else {
            panic!("the stream didn't fail: {events:?}");
        };
        assert_eq!(error.code, "provider_unreachable", "{kind}");
        assert!(error.retryable);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_provider_without_a_key_asks_for_one_and_sends_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let ai = AiFoundation::open_with(
        folder.path().join("providers.json"),
        Arc::new(MemoryCredentials::new()),
    )
    .unwrap();
    ai.save_provider(AiProviderConfig {
        id: "local".into(),
        kind: "openai".into(),
        display_name: "OpenAI".into(),
        model: "test-model".into(),
        endpoint: None,
        credential_ref: None,
        enabled: true,
        context_window: None,
    })
    .unwrap();
    let events = events(stream(&ai, vec![text("user", "Hello.")])).await;
    let Some(AiStreamEvent::Error { error }) = events.last() else {
        panic!("the stream didn't fail: {events:?}");
    };
    assert_eq!(error.code, "provider_key_missing");
}

/// Nothing a provider answers changes the code into something else: a
/// body that isn't JSON, or JSON of another shape, still gets one.
#[tokio::test(flavor = "multi_thread")]
async fn an_odd_body_still_gets_a_code_from_its_status() {
    for (status, body, code) in [
        (
            "401 Unauthorized",
            "<html>secret-provider-words</html>",
            "provider_auth_failed",
        ),
        ("418 I'm a teapot", "[1, 2, 3]", "provider_rejected"),
        ("502 Bad Gateway", "", "provider_overloaded"),
    ] {
        let error = failure("anthropic", Answer::Status(status, body)).await;
        assert_eq!(error.code, code, "{status}");
    }
}
