//! Presence and the server's logs (sync-v2.md §23): nothing a presence
//! record carries reaches them. A test binary of its own, because it takes
//! over the process's log output.

mod sync_support;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::sync::realtime::Channel;
use app_core::sync::workspace::presence::SelectionRange;
use base64::Engine;
use sync_protocol::messages::Frame;
use sync_support::pair;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_servers_logs_hold_nothing_of_a_presence_record() {
    let logs = Logs::default();
    let writer = logs.clone();
    // The server's crates at every level. The WebSocket library traces the
    // bytes it handles in this process, on the client's side too.
    let server: Targets = "sync_server=trace,sync_store=trace,sync_store_sqlite=trace"
        .parse()
        .expect("targets");
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(server),
        )
        .init();
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

    let logged = logs.0.lock().unwrap().clone();
    let logged = String::from_utf8_lossy(&logged);
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
