//! Continue with ChatGPT in the app, against a fake of OpenAI on this
//! computer: the browser opens on the sign-in, the ChatGPT provider
//! appears once you come back, a message on the AI page uses the plan,
//! the page says so, and sign-out ends the session. The tokens never
//! reach a file, a provider row or the page.

use crate::support;

use local_core::chatgpt_fake::{EMAIL, FakeOpenAi};
use support::{Call, FakeSystem, Fixture, ai, plugins};

/// Every file under `folder`, as text.
fn files(folder: &std::path::Path) -> String {
    let mut all = String::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            all.push_str(&files(&path));
        } else {
            all.push_str(&String::from_utf8_lossy(
                &std::fs::read(&path).unwrap_or_default(),
            ));
        }
    }
    all
}

/// The sign-in address the browser opened, once it opened one.
fn opened(system: &FakeSystem, issuer: &str) -> Option<String> {
    system.calls().into_iter().find_map(|call| match call {
        Call::OpenUrl(url) if url.starts_with(issuer) => Some(url),
        _ => None,
    })
}

#[test]
fn continue_with_chatgpt_signs_in_and_a_message_uses_your_plan() {
    let fake = FakeOpenAi::start();
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.set_chatgpt_endpoints(fake.endpoints.clone());
    assert!(!app.chatgpt_status().signed_in);

    app.sign_in_with_chatgpt();
    let issuer = fake.endpoints.issuer.clone();
    plugins::wait_until(&mut app, "the browser opens", |_| {
        opened(&system, &issuer).is_some()
    });
    assert!(app.chatgpt_status().waiting);
    let url = opened(&system, &issuer).unwrap();
    assert_eq!(fake.come_back(&url), 200);
    plugins::wait_until(&mut app, "the sign-in finishes", |app| {
        let status = app.chatgpt_status();
        status.signed_in && !status.waiting
    });
    let status = app.chatgpt_status();
    assert!(!status.waiting);
    assert_eq!(status.problem, None);
    assert_eq!(status.email.as_deref(), Some(EMAIL));
    assert!(
        status.first_sign_in,
        "the first sign-in says Noura uses your plan"
    );
    let row = app
        .ai_providers()
        .into_iter()
        .find(|row| row.id == "chatgpt")
        .expect("the ChatGPT provider is set up");
    assert_eq!(row.kind, "chatgpt");
    // The plan's catalog has no gpt-6-luna, so its first model answers.
    assert_eq!(row.model, "gpt-6.2-sol");
    assert!(row.has_key && row.enabled);

    app.dismiss_chatgpt_notice();
    assert!(!app.chatgpt_status().first_sign_in);

    // A message on the AI page goes to the plan with its access token.
    let Some(folder) = ai::folder() else {
        return;
    };
    plugins::install_and_enable(&mut app, folder.path(), "ai");
    plugins::wait_until(&mut app, "the AI page shows", |app| {
        app.plugin_view(ai::PAGE).is_some()
    });
    ai::open_panel(&mut app);
    ai::send(&mut app, "Hello", "chatgpt");
    ai::wait_until(&mut app, "the answer shows", |app| {
        ai::lines(app)
            .iter()
            .any(|line| line.contains("Adding it."))
    });
    assert!(
        ai::lines(&app)
            .iter()
            .any(|line| line.contains("Using ChatGPT plan") && line.contains("Manage usage")),
        "{:?}",
        ai::lines(&app)
    );
    let (authorization, body) = fake.state().responses[0].clone();
    assert_eq!(authorization, "Bearer access_1");
    assert_eq!(body["model"], "gpt-6.2-sol");
    assert_eq!(body["store"], false);

    // The tokens reach neither a file, a provider row nor the page.
    let everywhere = format!(
        "{}{}{:?}{:?}",
        files(fixture.app_data.path()),
        files(&fixture.root),
        app.ai_providers(),
        ai::lines(&app)
    );
    for secret in ["access_1", "refresh_1", "eyJ"] {
        assert!(!everywhere.contains(secret), "{secret} leaked");
    }

    app.sign_out_chatgpt();
    plugins::wait_until(&mut app, "the sign-out finishes", |app| {
        !app.chatgpt_status().signed_in
    });
    assert_eq!(app.chatgpt_status().problem, None);
    assert_eq!(fake.state().revoked.len(), 1);
    assert!(system.credentials.secrets().is_empty());
}

#[test]
fn declining_in_the_browser_says_so_and_sets_nothing_up() {
    let fake = FakeOpenAi::start();
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.set_chatgpt_endpoints(fake.endpoints.clone());
    app.sign_in_with_chatgpt();
    let issuer = fake.endpoints.issuer.clone();
    plugins::wait_until(&mut app, "the browser opens", |_| {
        opened(&system, &issuer).is_some()
    });
    fake.decline(&opened(&system, &issuer).unwrap());
    plugins::wait_until(&mut app, "the sign-in ends", |app| {
        !app.chatgpt_status().waiting
    });
    let status = app.chatgpt_status();
    assert!(!status.signed_in);
    assert_eq!(
        status.problem.as_deref(),
        Some("You declined in ChatGPT, so Noura isn't using your plan.")
    );
    assert!(app.ai_providers().is_empty());
}

#[test]
fn cancelling_a_sign_in_stops_waiting_for_the_browser() {
    let fake = FakeOpenAi::start();
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.set_chatgpt_endpoints(fake.endpoints.clone());
    app.sign_in_with_chatgpt();
    let issuer = fake.endpoints.issuer.clone();
    plugins::wait_until(&mut app, "the browser opens", |_| {
        opened(&system, &issuer).is_some()
    });
    app.cancel_chatgpt_sign_in();
    plugins::wait_until(&mut app, "the sign-in stops", |app| {
        !app.chatgpt_status().waiting
    });
    let status = app.chatgpt_status();
    assert!(!status.signed_in);
    assert_eq!(status.problem, None);
}
