//! Seam 1: a chat's life after its first answer. You rename it, and an
//! edit made elsewhere wins. You keep it until you delete it or have it
//! removed after 30 days, and Noura moves an expired chat to the trash
//! only when its folder holds nothing but its own unchanged files.

mod support;

use std::{sync::Arc, time::Duration};

use app_core::App;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins,
};

/// A workspace with the AI plugin on and the provider Local allowed. The
/// app comes first, so it closes before its folders go.
struct Chatting {
    app: App,
    provider: Arc<FakeProvider>,
    id: String,
    _plugin: plugins::PluginFolder,
    fixture: Fixture,
}

impl Chatting {
    fn new() -> Option<Self> {
        let fixture = Fixture::new(&[]);
        let mut app = fixture.app_with(FakeSystem::new());
        let provider = FakeProvider::new();
        let plugin = ai::with_ai(&mut app, &provider)?;
        let id = app
            .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
        app.grant_ai_consent(&id).unwrap();
        support::settle(&mut app);
        Some(Self {
            app,
            provider,
            id,
            _plugin: plugin,
            fixture,
        })
    }

    /// Chat once, and return the chat's ID.
    fn chat(&mut self) -> String {
        let provider = self.provider.clone();
        ai::chat_once(
            &mut self.app,
            &provider,
            &self.id,
            "Plan the garden",
            "Plant garlic.",
        );
        ai::chat_ids(&self.app)[0].clone()
    }

    fn wait(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        ai::wait_until(&mut self.app, what, done);
    }

    /// The `chat.md`'s path and text, once `done` holds for its text.
    fn chat_file_once(&mut self, what: &str, done: impl Fn(&str) -> bool) -> (String, String) {
        let root = self.fixture.root.clone();
        self.wait(what, |_| done(&ai::chat_file(&root).1));
        ai::chat_file(&root)
    }
}

#[test]
fn renaming_a_chat_writes_its_title_and_an_edit_made_elsewhere_wins() {
    let Some(mut chatting) = Chatting::new() else {
        return;
    };
    let chat = chatting.chat();
    let (path, _) = ai::chat_file(&chatting.fixture.root);

    ai::choose(&mut chatting.app, &format!("rename:{chat}:"));
    assert_eq!(
        ai::name_field(&chatting.app).as_deref(),
        Some("Plan the garden")
    );
    ai::name_chat(&mut chatting.app, "Garlic plans");

    chatting.wait("the sidebar shows the new title", |app| {
        ai::chat_titles(app) == ["Garlic plans"]
    });
    let (renamed, text) = ai::chat_file(&chatting.fixture.root);
    assert_eq!(
        ai::frontmatter(&text, "title").as_deref(),
        Some("Garlic plans")
    );
    assert_eq!(renamed, path, "the chat's folder stays where it is");

    // Another app edits the chat after you chose Rename.
    ai::choose(&mut chatting.app, &format!("rename:{chat}:"));
    let edited = text.replace("title: Garlic plans", "title: Edited elsewhere");
    std::fs::write(chatting.fixture.root.join(&path), &edited).unwrap();
    ai::name_chat(&mut chatting.app, "Mine");

    chatting.wait("the page reports the conflict", |app| {
        ai::lines(app)
            .contains(&"The chat changed elsewhere. Showing the latest version.".to_owned())
    });
    assert_eq!(ai::chat_file(&chatting.fixture.root).1, edited);
    chatting.wait("the sidebar shows the edit", |app| {
        ai::chat_titles(app) == ["Edited elsewhere"]
    });
}

#[test]
fn a_new_chat_stays_until_you_delete_it_and_switches_to_30_days_and_back() {
    let Some(mut chatting) = Chatting::new() else {
        return;
    };
    let chat = chatting.chat();
    let (_, text) = ai::chat_file(&chatting.fixture.root);
    assert_eq!(
        ai::frontmatter(&text, "retention").as_deref(),
        Some("permanent")
    );
    assert_eq!(
        ai::menu_item(&chatting.app, &format!("keep:{chat}:")).map(|(_, enabled)| enabled),
        Some(false),
        "the chat already stays until you delete it"
    );

    ai::choose(&mut chatting.app, &format!("ephemeral:{chat}:"));
    let (_, text) = chatting.chat_file_once("the chat expires after 30 days", |text| {
        text.contains("retention: ephemeral")
    });
    assert_eq!(
        ai::frontmatter(&text, "retention_days").as_deref(),
        Some("30")
    );
    chatting.wait("the sidebar says so", |app| {
        ai::chat_details(app) == [Some("Removed after 30 days".to_owned())]
    });
    assert!(
        ai::lines(&chatting.app)
            .contains(&"Noura moves this chat to the trash 30 days after it started.".to_owned())
    );

    ai::choose(&mut chatting.app, &format!("keep:{chat}:"));
    let (_, text) = chatting.chat_file_once("the chat stays again", |text| {
        text.contains("retention: permanent")
    });
    assert_eq!(
        ai::frontmatter(&text, "retention_days").as_deref(),
        Some("null")
    );
    chatting.wait("the sidebar says so", |app| ai::chat_details(app) == [None]);
}

/// An ephemeral chat's `chat.md`, started on 1 September and changed last
/// at `updated`.
fn ephemeral_chat(suffix: &str, title: &str, updated: &str) -> Vec<u8> {
    format!(
        "---\nid: chat_{suffix}\ntype: chat\ntitle: {title}\nretention: ephemeral\n\
         retention_days: 30\ncreated: 2026-09-01T10:00:00Z\nupdated: {updated}\n---\n"
    )
    .into_bytes()
}

/// A message of the chat `chat`, written on 1 September.
fn message(suffix: &str, chat: &str) -> Vec<u8> {
    format!(
        "---\nid: chat-message_{suffix}\ntype: chat-message\nchat_id: chat_{chat}\nrun_id: run_1\n\
         kind: user\nstatus: completed\ncontent_type: text/markdown\nprovider_id: null\n\
         model_id: null\ntool_call_id: null\ntool_name: null\nerror_code: null\n\
         summarizes_through_message_id: null\ncreated: 2026-09-01T10:01:00Z\n\
         updated: 2026-09-01T10:01:00Z\n---\n\nPlant garlic?\n"
    )
    .into_bytes()
}

/// Set the time `relative` last changed to `instant`.
fn touch(root: &std::path::Path, relative: &str, instant: &str) {
    let instant: jiff::Timestamp = instant.parse().unwrap();
    let time = std::time::UNIX_EPOCH + Duration::from_secs(instant.as_second() as u64);
    std::fs::File::options()
        .write(true)
        .open(root.join(relative))
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn an_expired_chat_goes_to_the_trash_only_when_its_folder_is_whole_and_unchanged() {
    const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
    const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";
    const C: &str = "01jbcdefghjkmnpqrstvwxyz0c";
    const D: &str = "01jbcdefghjkmnpqrstvwxyz0d";
    const E: &str = "01jbcdefghjkmnpqrstvwxyz0e";
    const F: &str = "01jbcdefghjkmnpqrstvwxyz0f";
    // Each chat's message takes the chat's ID with a 1 for its 0.
    let message_path = |folder: &str, chat: &str| {
        let suffix = chat.replace("z0", "z1");
        (
            format!("{folder}/messages/2026-09-01/chat-message_{suffix}.md"),
            message(&suffix, chat),
        )
    };
    // The whole, unchanged chat changed last, so the expiry looks at it
    // after every other chat.
    let mut files = vec![
        (
            "chats/expired--wxyz0a/chat.md".to_owned(),
            ephemeral_chat(A, "Expired", "2026-09-01T10:01:00Z"),
        ),
        message_path("chats/expired--wxyz0a", A),
        (
            "chats/unknown--wxyz0b/chat.md".to_owned(),
            ephemeral_chat(B, "Unknown file", "2026-09-01T10:02:00Z"),
        ),
        message_path("chats/unknown--wxyz0b", B),
        (
            "chats/unknown--wxyz0b/notes.txt".to_owned(),
            b"Mine.".to_vec(),
        ),
        (
            "chats/malformed--wxyz0c/chat.md".to_owned(),
            ephemeral_chat(C, "Malformed", "2026-09-01T10:03:00Z"),
        ),
        (
            format!(
                "chats/malformed--wxyz0c/messages/2026-09-01/chat-message_{}.md",
                C.replace("z0", "z1")
            ),
            b"---\nid: [broken\n---\n".to_vec(),
        ),
        (
            "archive/moved--wxyz0d/chat.md".to_owned(),
            ephemeral_chat(D, "Moved", "2026-09-01T10:04:00Z"),
        ),
        message_path("archive/moved--wxyz0d", D),
        (
            "chats/changed--wxyz0e/chat.md".to_owned(),
            ephemeral_chat(E, "Changed", "2026-09-01T10:05:00Z"),
        ),
        message_path("chats/changed--wxyz0e", E),
    ];
    if cfg!(unix) {
        files.push((
            "chats/linked--wxyz0f/chat.md".to_owned(),
            ephemeral_chat(F, "Linked", "2026-09-01T10:06:00Z"),
        ));
    }
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.outside(),
        fixture.root.join("chats/linked--wxyz0f/elsewhere"),
    )
    .unwrap();
    for (path, _) in &files {
        touch(&fixture.root, path, "2026-09-02T10:00:00Z");
    }
    // Changed after the chat expired on 1 October.
    touch(
        &fixture.root,
        &message_path("chats/changed--wxyz0e", E).0,
        "2026-10-05T10:00:00Z",
    );
    let system = FakeSystem::new();
    system.set_clock("2026-10-24T12:00:00Z", "America/Toronto");

    let mut app = fixture.app_with(system.clone());

    ai::wait_until(&mut app, "the expired chat goes", |_| {
        !fixture.exists("chats/expired--wxyz0a")
    });
    assert!(system.in_bin("expired--wxyz0a").join("chat.md").exists());
    for kept in [
        "chats/unknown--wxyz0b/notes.txt",
        "chats/unknown--wxyz0b/chat.md",
        "chats/malformed--wxyz0c/chat.md",
        "archive/moved--wxyz0d/chat.md",
        "chats/changed--wxyz0e/chat.md",
    ] {
        assert!(fixture.exists(kept), "{kept} stays");
    }
    if cfg!(unix) {
        assert!(fixture.exists("chats/linked--wxyz0f/chat.md"));
    }
}

#[test]
fn chats_that_haven_t_expired_yet_stay() {
    const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
    let fixture = Fixture::new(&[(
        "chats/recent--wxyz0a/chat.md",
        &ephemeral_chat(A, "Recent", "2026-09-01T10:00:00Z"),
    )]);
    touch(
        &fixture.root,
        "chats/recent--wxyz0a/chat.md",
        "2026-09-01T10:00:00Z",
    );
    let system = FakeSystem::new();
    system.set_clock("2026-09-30T12:00:00Z", "America/Toronto");

    let mut app = fixture.app_with(system.clone());
    std::thread::sleep(Duration::from_millis(300));
    support::settle(&mut app);

    assert!(fixture.exists("chats/recent--wxyz0a/chat.md"));
    assert!(system.calls().is_empty(), "{:?}", system.calls());
}
