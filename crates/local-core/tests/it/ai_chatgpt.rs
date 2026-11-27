//! Sign in with ChatGPT against a fake of OpenAI's authorization server
//! and Responses API on this computer: registration, the callback, the
//! code exchange with PKCE, the ID token's checks, keychain-only tokens,
//! refresh with rotation under a lock, a retry after a refused token, and
//! sign-out with revocation.

use std::{sync::Arc, time::Duration};

use local_core::{
    AiContentPart, AiFoundation, AiModelRef, AiStreamEvent, AiStreamInput, AiToolDefinition,
    MemoryCredentials,
    chatgpt::{ChatGptEndpoints, ChatGptRegistration, ChatGptSignIn},
    chatgpt_fake::{CLIENT, EMAIL, FakeOpenAi, SUBJECT, query},
};
use serde_json::json;

use super::ai_provider::{events, text};

const PATIENCE: Duration = Duration::from_secs(20);

/// Sign in as a browser would, through the fake.
async fn sign_in(
    fake: &Arc<FakeOpenAi>,
    registration: &ChatGptRegistration,
) -> Result<local_core::chatgpt::ChatGptCredential, String> {
    let sign_in = ChatGptSignIn::start(fake.endpoints.clone(), registration)
        .await
        .unwrap();
    let url = sign_in.url().to_owned();
    let browser = {
        let fake = fake.clone();
        tokio::task::spawn_blocking(move || fake.come_back(&url))
    };
    let finished = sign_in.finish(PATIENCE).await;
    assert_eq!(browser.await.unwrap(), 200);
    finished.map_err(|error| error.code)
}

fn foundation(
    endpoints: &ChatGptEndpoints,
) -> (tempfile::TempDir, Arc<MemoryCredentials>, AiFoundation) {
    let folder = tempfile::tempdir().unwrap();
    let credentials = Arc::new(MemoryCredentials::new());
    let ai = AiFoundation::open_with(folder.path().join("ai/providers.json"), credentials.clone())
        .unwrap()
        .with_chatgpt_endpoints(endpoints.clone());
    (folder, credentials, ai)
}

fn ask(ai: &AiFoundation, operation: &str) -> local_core::AiStreamOperation {
    ai.start_stream(AiStreamInput {
        operation_id: operation.into(),
        model: AiModelRef {
            provider_id: "chatgpt".into(),
            model: "gpt-6-luna".into(),
        },
        messages: vec![text("system", "Be brief."), text("user", "Add a task to plant.")],
        tools: Some(vec![AiToolDefinition {
            name: "add_task".into(),
            description: Some("Add a task.".into()),
            input_schema: json!({ "type": "object", "properties": { "title": { "type": "string" } } }),
        }]),
        reasoning: None,
    })
    .unwrap()
}

/// Every file under `folder`, as text.
fn files(folder: &std::path::Path) -> String {
    let mut all = String::new();
    for entry in walk(folder) {
        all.push_str(&std::fs::read_to_string(entry).unwrap_or_default());
    }
    all
}

fn walk(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn you_sign_in_with_chatgpt_and_requests_use_your_plan() {
    let fake = Arc::new(FakeOpenAi::start());
    fake.state().tool_call = true;
    let endpoints = fake.endpoints.clone();
    let (folder, credentials, ai) = foundation(&endpoints);
    let registration = ai.chatgpt_registration().unwrap();
    assert!(registration.ext_agent_host_id.starts_with("urn:uuid:"));
    assert_eq!(
        ai.chatgpt_registration().unwrap(),
        registration,
        "the host ID stays"
    );

    let sign_in = ChatGptSignIn::start(endpoints.clone(), &registration)
        .await
        .unwrap();
    let asked = query(sign_in.url());
    assert!(
        sign_in
            .url()
            .starts_with(&format!("{}/api/accounts/authorize?", endpoints.issuer))
    );
    assert_eq!(asked["client_id"], "dynamic_agent_client");
    assert_eq!(asked["agent_name_hint"], "Noura");
    assert_eq!(asked["ext_agent_host_id"], registration.ext_agent_host_id);
    assert_eq!(asked["response_type"], "code");
    assert_eq!(asked["code_challenge_method"], "S256");
    assert_eq!(asked["resource"], "https://api.openai.com/v1");
    assert!(asked["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(asked["redirect_uri"].ends_with("/auth/callback"));
    assert!(asked["scope"].contains("chatgpt.tokens.use.direct"));
    drop(sign_in);

    let credential = sign_in_ok(&fake, &registration).await;
    assert_eq!(credential.client_id, CLIENT);
    assert_eq!(credential.subject, SUBJECT);
    assert_eq!(credential.email.as_deref(), Some(EMAIL));
    let id = ai.save_chatgpt(&credential, "gpt-6-luna").unwrap();
    assert_eq!(id, "chatgpt");
    let saved = ai.chatgpt_registration().unwrap();
    assert_eq!(saved.client_id.as_deref(), Some(CLIENT));
    assert_eq!(saved.ext_agent_host_id, registration.ext_agent_host_id);

    let events = events(ask(&ai, "01890a5d-ac96-7000-8000-0000000000c1")).await;
    assert!(
        matches!(events.last(), Some(AiStreamEvent::Completed { .. })),
        "{events:?}"
    );
    let calls: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::ToolCall { call } => Some(call.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "add_task");
    assert_eq!(calls[0].arguments, json!({ "title": "Plant" }));

    let (authorization, body) = fake.state().responses[0].clone();
    assert_eq!(authorization, "Bearer access_1");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["instructions"], "Be brief.");
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["tools"][0]["name"], "add_task");
    let input = body["input"].as_array().unwrap();
    assert!(
        input.iter().all(|item| item["role"] != "system"),
        "{input:?}"
    );
    for field in [
        "max_output_tokens",
        "temperature",
        "previous_response_id",
        "metadata",
    ] {
        assert!(body.get(field).is_none(), "{field}");
    }

    // The tokens live in the credential store and nowhere in the files.
    let on_disk = files(folder.path());
    for secret in ["access_1", "refresh_1", "eyJ"] {
        assert!(!on_disk.contains(secret), "{secret} reached a file");
    }
    let held = credentials.secrets().join("");
    assert!(held.contains("refresh_1") && held.contains("access_1"));

    // The next request sends the tool result after the call, in its
    // namespace.
    let follow_up = ai
        .start_stream(AiStreamInput {
            operation_id: "01890a5d-ac96-7000-8000-0000000000c2".into(),
            model: AiModelRef {
                provider_id: "chatgpt".into(),
                model: "gpt-6-luna".into(),
            },
            messages: vec![
                text("user", "Add a task to plant."),
                local_core::AiTransportMessage {
                    role: "assistant".into(),
                    content: vec![AiContentPart::ToolCall {
                        call_id: "call_1".into(),
                        name: "add_task".into(),
                        arguments: json!({ "title": "Plant" }),
                    }],
                },
                local_core::AiTransportMessage {
                    role: "tool".into(),
                    content: vec![AiContentPart::ToolResult {
                        call_id: "call_1".into(),
                        name: Some("add_task".into()),
                        content: "Added.".into(),
                    }],
                },
            ],
            tools: None,
            reasoning: None,
        })
        .unwrap();
    super::ai_provider::events(follow_up).await;
    let (_, body) = fake.state().responses[1].clone();
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][1]["namespace"], "noura");
    assert_eq!(body["input"][2]["type"], "function_call_output");
    assert_eq!(body["input"][2]["output"], "Added.");

    assert_eq!(
        ai.chatgpt_models(&ai.chatgpt_access_token().await.unwrap())
            .await
            .unwrap(),
        [("gpt-6.2-sol".to_owned(), "GPT-6.2 Sol".to_owned())]
    );

    // Sign-out ends the renewable session and forgets the tokens, and the
    // registration stays for the next sign-in.
    assert!(ai.sign_out_chatgpt().await.unwrap());
    assert_eq!(
        fake.state().revoked,
        [("refresh_1".to_owned(), CLIENT.to_owned())]
    );
    assert!(credentials.secrets().is_empty());
    assert!(ai.list_providers().unwrap().is_empty());
    assert_eq!(
        ai.chatgpt_registration().unwrap().client_id.as_deref(),
        Some(CLIENT)
    );
}

async fn sign_in_ok(
    fake: &Arc<FakeOpenAi>,
    registration: &ChatGptRegistration,
) -> local_core::chatgpt::ChatGptCredential {
    sign_in(fake, registration).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_returning_sign_in_reuses_the_client_it_registered() {
    let fake = FakeOpenAi::start();
    let endpoints = fake.endpoints.clone();
    let registration = ChatGptRegistration {
        client_id: Some(CLIENT.into()),
        email: Some(EMAIL.into()),
        ..ChatGptRegistration::new()
    };
    let sign_in = ChatGptSignIn::start(endpoints, &registration)
        .await
        .unwrap();
    let asked = query(sign_in.url());
    assert_eq!(asked["client_id"], CLIENT);
    assert_eq!(asked["login_hint"], EMAIL);
    assert!(!asked.contains_key("agent_name_hint"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expiring_token_refreshes_once_for_requests_at_the_same_time() {
    let fake = Arc::new(FakeOpenAi::start());
    let endpoints = fake.endpoints.clone();
    let (_folder, credentials, ai) = foundation(&endpoints);
    // The first tokens last a second, so the next request replaces them.
    fake.state().expires_in = 1;
    let credential = sign_in_ok(&fake, &ai.chatgpt_registration().unwrap()).await;
    ai.save_chatgpt(&credential, "gpt-6-luna").unwrap();
    fake.state().expires_in = 3600;

    let first = ask(&ai, "01890a5d-ac96-7000-8000-0000000000d1");
    let second = ask(&ai, "01890a5d-ac96-7000-8000-0000000000d2");
    let (first, second) = tokio::join!(events(first), events(second));
    assert!(
        matches!(first.last(), Some(AiStreamEvent::Completed { .. })),
        "{first:?}"
    );
    assert!(
        matches!(second.last(), Some(AiStreamEvent::Completed { .. })),
        "{second:?}"
    );
    let fake = fake.state();
    assert_eq!(fake.refreshes, 1, "one refresh for both requests");
    assert!(
        fake.responses
            .iter()
            .all(|(authorization, _)| authorization == "Bearer access_2")
    );
    // The rotated refresh token replaced the old one in the store.
    let held = credentials.secrets().join("");
    assert!(held.contains("refresh_2") && !held.contains("refresh_1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_access_token_is_refreshed_and_the_request_tried_again() {
    let fake = Arc::new(FakeOpenAi::start());
    let endpoints = fake.endpoints.clone();
    let (_folder, _credentials, ai) = foundation(&endpoints);
    let credential = sign_in_ok(&fake, &ai.chatgpt_registration().unwrap()).await;
    ai.save_chatgpt(&credential, "gpt-6-luna").unwrap();
    // OpenAI stops taking the access token before it expires.
    fake.state().access = "revoked".into();
    let events = events(ask(&ai, "01890a5d-ac96-7000-8000-0000000000e1")).await;
    assert!(
        matches!(events.last(), Some(AiStreamEvent::Completed { .. })),
        "{events:?}"
    );
    let fake = fake.state();
    assert_eq!(fake.refreshes, 1);
    let used: Vec<&str> = fake
        .responses
        .iter()
        .map(|(auth, _)| auth.as_str())
        .collect();
    assert_eq!(used, ["Bearer access_1", "Bearer access_2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_openai_refuses_asks_you_to_sign_in_again() {
    let fake = Arc::new(FakeOpenAi::start());
    let endpoints = fake.endpoints.clone();
    let (_folder, _credentials, ai) = foundation(&endpoints);
    fake.state().expires_in = 1;
    let credential = sign_in_ok(&fake, &ai.chatgpt_registration().unwrap()).await;
    ai.save_chatgpt(&credential, "gpt-6-luna").unwrap();
    fake.state().refuse_refresh = true;
    let events = events(ask(&ai, "01890a5d-ac96-7000-8000-0000000000f1")).await;
    let Some(AiStreamEvent::Error { error }) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(error.code, "provider_signed_out");
    let shown = serde_json::to_string(error).unwrap();
    assert!(
        !shown.contains("refresh_1") && !shown.contains("access_1"),
        "{shown}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_whose_id_token_doesnt_check_out_keeps_nothing() {
    for wrong in ["nonce", "audience", "signature"] {
        let fake = Arc::new(FakeOpenAi::start());
        fake.state().wrong = Some(wrong);
        let Err(error) = sign_in(&fake, &ChatGptRegistration::new()).await else {
            panic!("a wrong {wrong} signed in");
        };
        assert_eq!(error, "chatgpt_id_token_invalid", "{wrong}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_without_the_plan_scope_says_so() {
    let fake = Arc::new(FakeOpenAi::start());
    fake.state().scope = "openid profile email offline_access".into();
    let Err(error) = sign_in(&fake, &ChatGptRegistration::new()).await else {
        panic!("a sign-in without the plan scope finished");
    };
    assert_eq!(error, "chatgpt_plan_not_granted");
}

#[tokio::test(flavor = "multi_thread")]
async fn declining_in_the_browser_ends_the_sign_in() {
    let fake = FakeOpenAi::start();
    let endpoints = fake.endpoints.clone();
    let sign_in = ChatGptSignIn::start(endpoints, &ChatGptRegistration::new())
        .await
        .unwrap();
    let url = sign_in.url().to_owned();
    let browser = tokio::task::spawn_blocking(move || fake.decline(&url));
    let error = sign_in.finish(PATIENCE).await.unwrap_err();
    assert_eq!(error.code, "chatgpt_sign_in_declined");
    assert_eq!(browser.await.unwrap(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_nobody_finishes_times_out() {
    let fake = FakeOpenAi::start();
    let endpoints = fake.endpoints.clone();
    let sign_in = ChatGptSignIn::start(endpoints, &ChatGptRegistration::new())
        .await
        .unwrap();
    let error = sign_in
        .finish(Duration::from_millis(200))
        .await
        .unwrap_err();
    assert_eq!(error.code, "chatgpt_sign_in_timed_out");
}
