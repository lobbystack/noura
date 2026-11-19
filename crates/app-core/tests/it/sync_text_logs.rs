//! Presence and the server's logs (sync-v2.md §23): nothing a presence
//! record carries reaches them.

use crate::support;
use crate::sync_support;
use std::time::Duration;

use app_core::sync::realtime::Channel;
use app_core::sync::workspace::presence::SelectionRange;
use base64::Engine;
use sync_protocol::messages::Frame;
use sync_support::pair;
use tracing_subscriber::filter::Targets;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_servers_logs_hold_nothing_of_a_presence_record() {
    // The server's crates at every level. The WebSocket library traces the
    // bytes it handles in this process, on the client's side too.
    let server: Targets = "sync_server=trace,sync_store=trace,sync_store_sqlite=trace"
        .parse()
        .expect("targets");
    let logs = support::logs::Capture::of(server);
    tracing::info!(target: "sync_server", "the logs start");

    let mut pair = pair().await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let (_, a) = pair.sync_a.open_text("Plan.md").expect("the note opens");
    assert!(a.is_some());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let (_, b) = pair.sync_b.open_text("Plan.md").expect("the note opens");
    assert!(b.is_some());
    let mut channel_a = Channel::open(pair.service.client(), pair.sync_a.workspace())
        .await
        .expect("the channel opens");
    let mut channel_b = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel opens");
    pair.sync_a.presence_connected().unwrap();
    pair.sync_b.presence_connected().unwrap();
    for frame in pair.sync_a.presence_frames().unwrap() {
        channel_a.send(&frame).await.unwrap();
    }
    for frame in pair.sync_b.presence_frames().unwrap() {
        channel_b.send(&frame).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    pair.sync_a
        .set_selections("Plan.md", vec![SelectionRange { anchor: 1, head: 2 }], true);
    let frames = pair.sync_a.presence_frames().unwrap();
    let Some(Frame::Presence { record }) = frames.last().cloned() else {
        panic!("a presence record");
    };
    channel_a
        .send(&Frame::Presence {
            record: record.clone(),
        })
        .await
        .unwrap();
    let arrived = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Frame::Presence { .. }) = channel_b.next().await {
                break;
            }
        }
    })
    .await;
    assert!(arrived.is_ok(), "the record crossed the server");
    drop(channel_a);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let logged = logs.text();
    assert!(!logged.is_empty(), "the test heard the logs");
    let ciphertext = &record.body.ciphertext.0;
    let forms = |bytes: &[u8]| {
        [
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
            format!("{:?}", &bytes[..8])
                .trim_end_matches(']')
                .to_owned(),
        ]
    };
    for form in forms(&ciphertext[..32])
        .into_iter()
        .chain(forms(&record.body.channel.0))
    {
        assert!(!logged.contains(&form), "the logs hold {form}");
    }
}
