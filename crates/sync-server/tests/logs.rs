//! The server's local log at seam 4 (§29): it keeps route templates,
//! request IDs and status codes, for at most 14 days, and never account
//! IDs, record bodies, tokens, `rid`s or ciphertext.

mod support;

use jiff::tz::TimeZone;
use sqlx::{ConnectOptions, Connection};
use support::{Client, Server, push_body, run, seed};
use sync_protocol::codec::{Bytes, Encode, List};
use sync_protocol::messages::{PullCursor, PullRequest, PushResponse};
use sync_protocol::types::{Digest, to_base64url, to_hex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_keep_routes_request_ids_and_statuses_for_14_days() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    let logs = data.join("logs");
    std::fs::create_dir_all(&logs).expect("log directory");
    let today = jiff::Timestamp::now().to_zoned(TimeZone::UTC).date();
    let kept = today
        .checked_sub(jiff::Span::new().days(13))
        .expect("a date");
    let expired = today
        .checked_sub(jiff::Span::new().days(14))
        .expect("a date");
    std::fs::write(logs.join(format!("server-{kept}.log")), "kept\n").expect("written");
    std::fs::write(logs.join(format!("server-{expired}.log")), "expired\n").expect("written");

    let account = seed(&data, 1).await;
    let created = account.creation();
    let op = created.op(&account.keys[0], 1, Digest::ZERO, |_| {});
    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let w = created.workspace.to_base64url();
    let _: PushResponse = client
        .post("/v2/workspaces", created.body.clone())
        .await
        .decode();
    let _: PushResponse = client
        .post(
            &format!("/v2/workspaces/{w}/ops"),
            push_body(std::slice::from_ref(&op)),
        )
        .await
        .decode();
    let refused = client
        .post(
            &format!("/v2/workspaces/{w}/ops"),
            push_body(std::slice::from_ref(&op)),
        )
        .await;
    assert!(refused.status.is_success(), "a retry gets its receipt");
    let conflicting = created.op(&account.keys[0], 1, Digest::ZERO, |o| {
        o.rid = op.body.rid;
        o.ciphertext = Bytes(vec![0x5b; 272]);
    });
    let refused = client
        .post(
            &format!("/v2/workspaces/{w}/ops"),
            push_body(&[conflicting]),
        )
        .await;
    assert_eq!(refused.status.as_u16(), 409);
    let pull = PullRequest {
        cursors: List(vec![PullCursor {
            domain: created.domain,
            after: 0,
        }]),
        wait_seconds: 0,
    };
    let _ = client
        .post(&format!("/v2/workspaces/{w}/pull"), pull.to_bytes())
        .await;
    let _ = client
        .get(&format!(
            "/v2/workspaces/{w}/domains/{}/heads/{}",
            created.domain.to_base64url(),
            account.keys[0].device().to_base64url()
        ))
        .await;
    let _ = client.get("/v1/limits#fragment-secret").await;
    let _ = Client::new(server.base(), None).get("/v2/workspaces").await;
    // A database failure: the response says `internal` and nothing more,
    // and the cause stays in the local log.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(data.join(sync_store_sqlite::DATABASE_FILE))
        .connect()
        .await
        .expect("the database opens");
    sqlx::query("DROP TABLE profiles")
        .execute(&mut connection)
        .await
        .expect("a table goes missing");
    connection.close().await.expect("closed");
    let failed = client.get("/v2/workspaces").await;
    assert_eq!(failed.status.as_u16(), 500);
    let error = failed.error();
    assert_eq!(error.code.as_str(), "internal");
    assert!(
        !error.message.as_str().contains("profiles"),
        "{}",
        error.message.as_str()
    );
    server.kill();

    assert!(
        !logs.join(format!("server-{expired}.log")).exists(),
        "older than 14 days"
    );
    assert!(
        logs.join(format!("server-{kept}.log")).exists(),
        "within 14 days"
    );
    let today_log =
        std::fs::read_to_string(logs.join(format!("server-{today}.log"))).expect("today's log");
    let lines: Vec<&str> = today_log.lines().collect();
    assert!(lines.len() >= 8, "{today_log}");

    let secrets = [
        to_hex(&account.account.0),
        to_base64url(&account.account.0),
        to_hex(&account.keys[0].device().0),
        to_base64url(&account.keys[0].device().0),
        to_hex(&op.body.rid.0),
        to_base64url(&op.body.rid.0),
        to_hex(&created.workspace.0),
        w.clone(),
        account.token(0),
        to_hex(&account.tokens[0]),
        "5a5a5a5a".to_owned(),
        "Bearer".to_owned(),
        "fragment-secret".to_owned(),
    ];
    for line in &lines {
        for secret in &secrets {
            assert!(
                !line.contains(secret.as_str()),
                "the log holds {secret}: {line}"
            );
        }
        if line.contains(" request ") || line.ends_with(" request") {
            assert!(line.contains("request="), "{line}");
            assert!(line.contains("status="), "{line}");
            let route = line
                .split_whitespace()
                .find_map(|word| word.strip_prefix("route="))
                .expect("a route");
            assert!(
                route == "unmatched" || route.starts_with("/v2/"),
                "a route template: {line}"
            );
        }
    }
    assert!(
        today_log.contains("route=/v2/workspaces/{w}/ops status=200"),
        "{today_log}"
    );
    assert!(
        today_log.contains("route=/v2/workspaces/{w}/ops status=409"),
        "{today_log}"
    );
    assert!(
        today_log.contains("route=unmatched status=404"),
        "{today_log}"
    );
    assert!(today_log.contains("status=401"), "{today_log}");
    assert!(
        today_log.contains("store failure") && today_log.contains("no such table: profiles"),
        "the cause of a store failure stays in the local log: {today_log}"
    );
    assert!(
        today_log.contains("route=/v2/workspaces status=500"),
        "{today_log}"
    );
}
