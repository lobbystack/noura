//! The device's end of the realtime channel (docs/workspace-format/
//! sync-v2.md §23): a WebSocket per workspace, opened with the session
//! token over the same HTTP client settings as every other route, that
//! brings notify, records and hint frames.
//!
//! [`Live`] keeps a folder in step with the channel: while it is up the
//! device applies records frames that follow straight on from its cursor,
//! pulls on any gap or notify, sends a ping every 20 seconds and still
//! makes one workspace pull a minute; while it is down the device
//! long-polls that pull and reconnects with jittered backoff between 1 and
//! 30 seconds. Frames only feed the checks that pulls go through, so a
//! relay that forges or drops them delays sync and changes nothing the
//! device accepts.

use std::ops::Range;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::{ErrorBody, Frame, MAX_FRAME};
use sync_protocol::types::Id;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Message, handshake::derive_accept_key};

use super::workspace::{PULL_WAIT_SECONDS, WorkspaceSync};
use super::{SyncClient, SyncError, SyncService, network, random};

/// How often the app pings the server while the channel is up.
pub const PING_EVERY: Duration = Duration::from_secs(20);
/// How often the app makes a workspace pull while the channel is up.
pub const PULL_EVERY: Duration = Duration::from_secs(60);
/// The range the reconnect backoff stays in.
pub const BACKOFF: Range<Duration> = Duration::from_secs(1)..Duration::from_secs(30);

/// An open realtime channel to one workspace.
pub struct Channel {
    socket: WebSocketStream<reqwest::Upgraded>,
    ping: tokio::time::Interval,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Channel")
    }
}

impl Channel {
    /// Opens `GET /v2/workspaces/{w}/realtime` with the client's session.
    /// A refusal carries the §26 error code, such as `forbidden` for a
    /// device that isn't active in the workspace.
    pub async fn open(client: &SyncClient, workspace: Id) -> Result<Self, SyncError> {
        Self::open_every(client, workspace, PING_EVERY).await
    }

    /// [`Channel::open`] with pings every `ping`.
    pub async fn open_every(
        client: &SyncClient,
        workspace: Id,
        ping: Duration,
    ) -> Result<Self, SyncError> {
        // The upgrade needs HTTP/1.1, which a server may not pick over TLS
        // unless asked for it alone.
        let http = reqwest::Client::builder()
            .http1_only()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(network)?;
        let key = base64::engine::general_purpose::STANDARD
            .encode(random(sync_protocol::crypto::random_array::<16>())?);
        let url = format!(
            "{}/v2/workspaces/{}/realtime",
            client.base,
            workspace.to_base64url()
        );
        let response = http
            .get(url)
            .bearer_auth(&client.token)
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", &key)
            .send()
            .await
            .map_err(network)?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(network)?;
            let error = ErrorBody::from_bytes(&bytes).map_err(network)?;
            return Err(SyncError::Refused {
                code: error.code.0,
                status,
                detail: error.detail.map(|d| d.0),
            });
        }
        let accepted = response
            .headers()
            .get("sec-websocket-accept")
            .and_then(|v| v.to_str().ok())
            == Some(derive_accept_key(key.as_bytes()).as_str());
        if !accepted {
            return Err(SyncError::Network(
                "the server answered the upgrade for another key".into(),
            ));
        }
        let upgraded = response.upgrade().await.map_err(network)?;
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_FRAME + 5);
        config.max_frame_size = Some(MAX_FRAME + 5);
        let socket = WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await;
        let mut ping = tokio::time::interval_at(Instant::now() + ping, ping);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self { socket, ping })
    }

    /// The next frame, pinging the server on the way. A message that isn't
    /// a frame is skipped; a closed channel is a network error.
    pub async fn next(&mut self) -> Result<Frame, SyncError> {
        loop {
            tokio::select! {
                _ = self.ping.tick() => {
                    self.socket
                        .send(Message::Ping(Vec::new().into()))
                        .await
                        .map_err(network)?;
                }
                message = self.socket.next() => match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Ok(frame) = Frame::from_bytes(&bytes) {
                            return Ok(frame);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(SyncError::Network("the realtime channel closed".into()));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(network(error)),
                },
            }
        }
    }

    /// Sends a frame, such as a subscribe once presence arrives.
    pub async fn send(&mut self, frame: &Frame) -> Result<(), SyncError> {
        self.socket
            .send(Message::Binary(frame.to_bytes().into()))
            .await
            .map_err(network)
    }
}

/// The reconnect backoff: each failed attempt waits about twice as long as
/// the one before, within [`BACKOFF`], at a random point of the upper half
/// so devices that lost one server don't come back together.
#[derive(Clone, Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The wait before the next attempt, after one more failure.
    pub fn next_wait(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let ceiling = BACKOFF
            .start
            .saturating_mul(1 << self.failures.min(5))
            .min(BACKOFF.end);
        let floor = (ceiling / 2).max(BACKOFF.start);
        let span = ceiling.saturating_sub(floor).as_millis() as u64;
        let draw = random(sync_protocol::crypto::random_array::<8>())
            .map(u64::from_le_bytes)
            .unwrap_or(0);
        floor + Duration::from_millis(if span == 0 { 0 } else { draw % (span + 1) })
    }

    /// The channel opened.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

/// What one [`Live::step`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Whether the channel was up when the step ended.
    pub connected: bool,
    /// Whether the step pulled.
    pub pulled: bool,
    /// Other devices' records the step applied, from a pull or a frame.
    pub applied: usize,
}

/// A folder's channel, its backoff and its pull schedule.
#[derive(Debug)]
pub struct Live {
    channel: Option<Channel>,
    backoff: Backoff,
    retry_at: Instant,
    pull_due: Instant,
    pull_every: Duration,
    ping_every: Duration,
}

impl Default for Live {
    fn default() -> Self {
        Self::new(PULL_EVERY, PING_EVERY)
    }
}

impl Live {
    /// A schedule that pulls every `pull_every` while the channel is up
    /// and pings every `ping_every`.
    pub fn new(pull_every: Duration, ping_every: Duration) -> Self {
        let now = Instant::now();
        Self {
            channel: None,
            backoff: Backoff::default(),
            retry_at: now,
            pull_due: now,
            pull_every,
            ping_every,
        }
    }

    /// Whether the channel is up.
    pub fn connected(&self) -> bool {
        self.channel.is_some()
    }

    /// Drops the channel, as a lost connection would; the next step
    /// reconnects after the backoff.
    pub fn disconnect(&mut self) {
        self.lost();
    }

    fn lost(&mut self) {
        self.channel = None;
        self.retry_at = Instant::now() + self.backoff.next_wait();
    }

    /// One step of a folder's sync. With the channel down it tries to
    /// reconnect once the backoff passed, and otherwise runs a round whose
    /// pull long-polls; after it connects, a round without waiting catches
    /// up on what the channel didn't see. With the channel up it waits for
    /// a frame, `wake` or the minute's pull, and runs a round for `wake` or
    /// the pull and the frame's handling for a frame.
    pub async fn step(
        &mut self,
        sync: &mut WorkspaceSync,
        service: &mut SyncService,
        wake: &tokio::sync::Notify,
    ) -> Result<Step, SyncError> {
        if self.channel.is_none() && Instant::now() >= self.retry_at {
            match Channel::open_every(service.client(), sync.workspace(), self.ping_every).await {
                Ok(channel) => {
                    self.channel = Some(channel);
                    self.backoff.reset();
                    self.pull_due = Instant::now();
                }
                Err(_) => self.retry_at = Instant::now() + self.backoff.next_wait(),
            }
        }
        let Some(channel) = self.channel.as_mut() else {
            // Down: the pull long-polls, and ends early for a reconnect.
            let wait = self
                .retry_at
                .saturating_duration_since(Instant::now())
                .as_secs()
                .clamp(1, u64::from(PULL_WAIT_SECONDS)) as u32;
            let round = sync.round_until(service, wait, wake).await?;
            return Ok(Step {
                connected: false,
                pulled: true,
                applied: round.applied,
            });
        };
        if Instant::now() >= self.pull_due {
            self.pull_due = Instant::now() + self.pull_every;
            let round = sync.round(service, 0).await?;
            return Ok(Step {
                connected: true,
                pulled: true,
                applied: round.applied,
            });
        }
        let frame = tokio::select! {
            frame = channel.next() => Some(frame),
            () = wake.notified() => None,
            () = tokio::time::sleep_until(self.pull_due) => None,
        };
        match frame {
            Some(Ok(frame)) => {
                let outcome = sync.on_frame(service, &frame).await?;
                Ok(Step {
                    connected: true,
                    pulled: outcome.pulled,
                    applied: outcome.applied,
                })
            }
            Some(Err(_)) => {
                self.lost();
                Ok(Step::default())
            }
            None => {
                self.pull_due = Instant::now() + self.pull_every;
                let round = sync.round(service, 0).await?;
                Ok(Step {
                    connected: true,
                    pulled: true,
                    applied: round.applied,
                })
            }
        }
    }
}
