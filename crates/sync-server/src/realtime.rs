//! `GET /v2/workspaces/{w}/realtime`, the realtime channel of §23: a
//! WebSocket for one device active in the workspace. After a push commits
//! the connection reads the new entries from the database, on this
//! instance or, in PostgreSQL mode, after another instance's `NOTIFY`, and
//! forwards those that fit one frame in a records frame, or else a notify
//! with the new head, at most one per domain every 250 ms. It also tells
//! the device that the policy, the key stream or a member's account chain
//! moved, and the restore generation when it opens. The only frames a
//! client sends are presence records and subscriptions: a checked record
//! goes to the other connected devices that read its domain and subscribed
//! to its channel, on any instance, and nothing stores it. When a
//! connection ends, the same devices get `presence_gone` for its sessions.
//! A device holds a bounded number of channels to a workspace, and a channel
//! whose client stays silent, pings included, closes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{
    Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::response::Response;
use sync_protocol::codec::{Decode, Encode, Text};
use sync_protocol::messages::{Frame, MAX_FRAME};
use sync_protocol::records::{Presence, Signed};
use sync_protocol::types::{Id, IdOrZero};
use sync_store::{Backend, Relayed, Session, Store, Subscription, Watch};
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::http::{ApiError, AppState, Segments, bearer, id, refused};
use crate::rate::{ChannelSlot, RateGate};

/// The shortest time between two notify frames for one domain on one
/// connection (§23).
pub const NOTIFY_EVERY: Duration = Duration::from_millis(250);
/// The length of the window presence records are counted in.
const PRESENCE_WINDOW: Duration = Duration::from_secs(1);
/// The most sessions of one connection the server remembers for
/// `presence_gone`. A device draws one session per connection (§23).
const MAX_SESSIONS: usize = 8;

/// The bearer token and the device it names.
pub(crate) struct Device {
    session: Session,
    token: String,
}

impl<B: Backend> FromRequestParts<AppState<B>> for Device {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?.to_owned();
        let crate::http::Auth(session) =
            crate::http::Auth::from_request_parts(parts, state).await?;
        Ok(Self { session, token })
    }
}

/// Opens the channel for a device active in the workspace, and refuses any
/// other with the error body of §26 before the upgrade.
pub(crate) async fn open<B: Backend>(
    State(state): State<AppState<B>>,
    device: Device,
    Segments(workspace): Segments<String>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let upgrade =
        upgrade.map_err(|_| refused("bad_request", "the realtime route takes a WebSocket"))?;
    let store = state.store.clone();
    // Subscribe before the first read, so no commit slips between them.
    let changes = store.hub().subscribe(workspace);
    let watch = store.realtime_watch(device.session, workspace).await?;
    let slot = state.rate.open_channel(device.session.device, workspace)?;
    let relayed = store.hub().subscribe_presence(workspace);
    let limit = MAX_FRAME + 5;
    let presence_per_second = store.limits().presence_per_second;
    let idle = state.rate.channel_idle();
    Ok(upgrade
        .max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| {
            Connection {
                store,
                device,
                workspace,
                watch,
                notifies: Notifies::default(),
                presence: PresenceState::new(presence_per_second),
                idle,
                _slot: slot,
            }
            .run(socket, changes, relayed)
        }))
}

/// The notify frames of one connection, at most one per domain every
/// [`NOTIFY_EVERY`]: a notify that comes sooner waits, and a later one for
/// the same domain replaces it.
#[derive(Default)]
struct Notifies {
    sent: HashMap<Id, Instant>,
    waiting: BTreeMap<Id, Frame>,
}

impl Notifies {
    /// Takes a notify for `domain`; returns it when it may go now.
    fn offer(&mut self, domain: Id, frame: Frame, now: Instant) -> Option<Frame> {
        match self.sent.get(&domain) {
            Some(sent) if *sent + NOTIFY_EVERY > now => {
                self.waiting.insert(domain, frame);
                None
            }
            _ => {
                self.waiting.remove(&domain);
                self.sent.insert(domain, now);
                Some(frame)
            }
        }
    }

    /// A records frame for `domain` went out, which tells the device more
    /// than a waiting notify: its `previous_seq` sends a device that missed
    /// the notify's entries to pull.
    fn forget(&mut self, domain: &Id) {
        self.waiting.remove(domain);
    }

    /// The waiting notifies whose time came.
    fn due(&mut self, now: Instant) -> Vec<Frame> {
        let ready: Vec<Id> = self
            .waiting
            .keys()
            .filter(|domain| {
                self.sent
                    .get(domain)
                    .is_none_or(|s| *s + NOTIFY_EVERY <= now)
            })
            .copied()
            .collect();
        ready
            .into_iter()
            .filter_map(|domain| {
                self.sent.insert(domain, now);
                self.waiting.remove(&domain)
            })
            .collect()
    }

    /// When the next waiting notify may go.
    fn next(&self) -> Option<Instant> {
        self.waiting
            .keys()
            .filter_map(|domain| self.sent.get(domain).map(|s| *s + NOTIFY_EVERY))
            .min()
    }
}

/// What a connection knows of presence: the channels it subscribed to, the
/// channels each of its sessions wrote on, and its count this second.
struct PresenceState {
    subscribed: BTreeSet<Id>,
    sessions: BTreeMap<Id, BTreeSet<(Id, Id)>>,
    limit: u32,
    window: Instant,
    count: u32,
}

impl PresenceState {
    fn new(limit: u32) -> Self {
        Self {
            subscribed: BTreeSet::new(),
            sessions: BTreeMap::new(),
            limit,
            window: Instant::now(),
            count: 0,
        }
    }

    /// Counts a record, and returns whether it stays within the limit.
    fn admit(&mut self, now: Instant) -> bool {
        if now >= self.window + PRESENCE_WINDOW {
            self.window = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= self.limit
    }

    /// Remembers that `session` wrote on `channel`, unless the connection
    /// already holds as many sessions as it may.
    fn note(&mut self, session: Id, domain: Id, channel: Id) -> bool {
        if !self.sessions.contains_key(&session) && self.sessions.len() >= MAX_SESSIONS {
            return false;
        }
        self.sessions
            .entry(session)
            .or_default()
            .insert((domain, channel));
        true
    }
}

struct Connection<B: Backend> {
    store: Arc<Store<B>>,
    device: Device,
    workspace: Id,
    watch: Watch,
    notifies: Notifies,
    presence: PresenceState,
    /// How long the client may stay silent.
    idle: Duration,
    /// The channel's place in its device's count.
    _slot: ChannelSlot,
}

/// Why a connection ends.
struct Closed;

impl<B: Backend> Connection<B> {
    async fn run(
        mut self,
        mut socket: WebSocket,
        mut changes: Subscription<tokio::sync::watch::Receiver<u64>>,
        mut relayed: Subscription<broadcast::Receiver<Relayed>>,
    ) {
        let mut generation = self.store.restore_generation();
        if send(
            &mut socket,
            &Frame::Restored {
                restore_generation: generation,
            },
        )
        .await
        .is_err()
        {
            return;
        }
        let mut heard = Instant::now();
        loop {
            let next = self.notifies.next();
            let step = tokio::select! {
                changed = changes.changed() => match changed {
                    Ok(()) => self.changed(&mut socket, &mut generation).await,
                    Err(_) => Err(Closed),
                },
                () = sleep_until(next) => self.send_due(&mut socket).await,
                relay = relayed.recv() => match relay {
                    Ok(relay) => self.forward(&mut socket, relay).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
                    Err(broadcast::error::RecvError::Closed) => Err(Closed),
                },
                () = tokio::time::sleep_until(heard + self.idle) => Err(Closed),
                message = socket.recv() => {
                    heard = Instant::now();
                    match message {
                        Some(Ok(Message::Binary(bytes))) => {
                            self.client_frame(&mut socket, &bytes).await
                        }
                        Some(Ok(Message::Text(_))) => send(&mut socket, &refusal(&[])).await,
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => Ok(()),
                        Some(Ok(Message::Close(_)) | Err(_)) | None => Err(Closed),
                    }
                }
            };
            if step.is_err() {
                self.say_gone();
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        }
    }

    /// A frame the device sent: a subscribe replaces its channels, and a
    /// presence record that passes its checks goes on to the others.
    async fn client_frame(&mut self, socket: &mut WebSocket, bytes: &[u8]) -> Result<(), Closed> {
        match Frame::from_bytes(bytes) {
            Ok(Frame::Subscribe { channels }) => {
                self.presence.subscribed = channels.0.into_iter().collect();
                Ok(())
            }
            Ok(Frame::Presence { record }) => match self.accept_presence(record.clone()).await {
                Ok(()) => Ok(()),
                Err(code) => send(socket, &presence_refusal(&record, code)).await,
            },
            _ => send(socket, &refusal(bytes)).await,
        }
    }

    async fn accept_presence(&mut self, record: Signed<Presence>) -> Result<(), String> {
        if !self.presence.admit(Instant::now()) {
            return Err("rate_limited".to_owned());
        }
        // The connection's own count keeps a flood from reaching the limiter,
        // and the limiter counts the device across its channels.
        let limiter = self.store.backend().limiter();
        RateGate::take_presence(&*limiter, self.device.session.device, self.presence.limit)
            .await
            .map_err(|error| match error {
                sync_store::Error::Refused(refusal) => refusal.code.to_owned(),
                sync_store::Error::Internal(_) => "internal".to_owned(),
            })?;
        let body = &record.body;
        let (domain, channel, device, session) =
            (body.domain, body.channel, body.device, body.session);
        self.store
            .check_presence(self.device.session, self.workspace, record.clone())
            .await
            .map_err(|error| match error {
                sync_store::Error::Refused(refusal) => refusal.code.to_owned(),
                sync_store::Error::Internal(_) => "internal".to_owned(),
            })?;
        if !self.presence.note(session, domain, channel) {
            return Err("rate_limited".to_owned());
        }
        self.store.relay_presence(
            self.workspace,
            Relayed {
                domain,
                channel,
                device,
                frame: Frame::Presence { record }.to_bytes(),
            },
        );
        Ok(())
    }

    /// Passes a relayed frame to the device when it reads the record's
    /// domain and subscribed to its channel. A device doesn't hear itself.
    async fn forward(&self, socket: &mut WebSocket, relayed: Relayed) -> Result<(), Closed> {
        let wanted = relayed.device != self.device.session.device
            && self.presence.subscribed.contains(&relayed.channel)
            && self.watch.heads.contains_key(&relayed.domain);
        if !wanted {
            return Ok(());
        }
        socket
            .send(Message::Binary(relayed.frame.into()))
            .await
            .map_err(|_| Closed)
    }

    /// Tells the others this connection's sessions ended.
    fn say_gone(&self) {
        let device = self.device.session.device;
        for (session, channels) in &self.presence.sessions {
            let frame = Frame::PresenceGone {
                device,
                session: *session,
            }
            .to_bytes();
            for (domain, channel) in channels {
                self.store.relay_presence(
                    self.workspace,
                    Relayed {
                        domain: *domain,
                        channel: *channel,
                        device,
                        frame: frame.clone(),
                    },
                );
            }
        }
    }

    /// Something committed in the workspace: reads what the device hasn't
    /// heard of yet and sends it. A session that ended or a device that
    /// left the workspace closes the channel.
    async fn changed(
        &mut self,
        socket: &mut WebSocket,
        generation: &mut u64,
    ) -> Result<(), Closed> {
        if self.store.session(&self.device.token).await.is_err() {
            return Err(Closed);
        }
        let frames = self
            .store
            .realtime_frames(self.device.session, self.workspace, &mut self.watch)
            .await
            .map_err(|_| Closed)?;
        let now = Instant::now();
        for frame in frames {
            let ready = match &frame {
                Frame::Notify { domain, .. } => self.notifies.offer(*domain, frame.clone(), now),
                Frame::Records { domain, .. } => {
                    self.notifies.forget(domain);
                    Some(frame)
                }
                _ => Some(frame),
            };
            if let Some(frame) = ready {
                send(socket, &frame).await?;
            }
        }
        let current = self.store.restore_generation();
        if current != *generation {
            *generation = current;
            send(
                socket,
                &Frame::Restored {
                    restore_generation: current,
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn send_due(&mut self, socket: &mut WebSocket) -> Result<(), Closed> {
        for frame in self.notifies.due(Instant::now()) {
            send(socket, &frame).await?;
        }
        Ok(())
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn send(socket: &mut WebSocket, frame: &Frame) -> Result<(), Closed> {
    socket
        .send(Message::Binary(frame.to_bytes().into()))
        .await
        .map_err(|_| Closed)
}

/// The error frame that refuses a presence record.
fn presence_refusal(record: &Signed<Presence>, code: String) -> Frame {
    Frame::Error {
        rejected_type: 2,
        domain: IdOrZero(record.body.domain.0),
        session: IdOrZero(record.body.session.0),
        sequence: record.body.sequence,
        code: Text(code),
        detail: None,
    }
}

/// The error frame that answers a client frame the server never takes: any
/// type but a presence record or a subscribe, a subscribe past the limit,
/// or bytes that aren't a frame.
fn refusal(bytes: &[u8]) -> Frame {
    let rejected_type = match Frame::from_bytes(bytes) {
        Ok(frame) => frame.code(),
        Err(_) => bytes.first().copied().unwrap_or(0),
    };
    Frame::Error {
        rejected_type,
        domain: IdOrZero([0; 16]),
        session: IdOrZero([0; 16]),
        sequence: 0,
        code: Text("bad_request".into()),
        detail: None,
    }
}
