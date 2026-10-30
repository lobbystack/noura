//! `GET /v2/workspaces/{w}/realtime`, the realtime channel of §23: a
//! WebSocket for one device active in the workspace. After a push commits
//! the connection reads the new entries from the database, on this
//! instance or, in PostgreSQL mode, after another instance's `NOTIFY`, and
//! forwards those that fit one frame in a records frame, or else a notify
//! with the new head, at most one per domain every 250 ms. It also tells
//! the device that the policy, the key stream or a member's account chain
//! moved, and the restore generation when it opens. It carries no writes:
//! every client frame gets an error frame until presence arrives
//! (ticket 55).

use std::collections::{BTreeMap, HashMap};
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
use sync_protocol::types::{Id, IdOrZero};
use sync_store::{Backend, Session, Store, Watch};
use tokio::time::Instant;

use crate::http::{ApiError, AppState, Segments, bearer, id, refused};

/// The shortest time between two notify frames for one domain on one
/// connection (§23).
pub const NOTIFY_EVERY: Duration = Duration::from_millis(250);

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
    State(store): State<Arc<Store<B>>>,
    device: Device,
    Segments(workspace): Segments<String>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let upgrade =
        upgrade.map_err(|_| refused("bad_request", "the realtime route takes a WebSocket"))?;
    // Subscribe before the first read, so no commit slips between them.
    let changes = store.hub().subscribe(workspace);
    let watch = store.realtime_watch(device.session, workspace).await?;
    let limit = MAX_FRAME + 5;
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
            }
            .run(socket, changes)
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

struct Connection<B: Backend> {
    store: Arc<Store<B>>,
    device: Device,
    workspace: Id,
    watch: Watch,
    notifies: Notifies,
}

/// Why a connection ends.
struct Closed;

impl<B: Backend> Connection<B> {
    async fn run(mut self, mut socket: WebSocket, mut changes: tokio::sync::watch::Receiver<u64>) {
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
        loop {
            let next = self.notifies.next();
            let step = tokio::select! {
                changed = changes.changed() => match changed {
                    Ok(()) => self.changed(&mut socket, &mut generation).await,
                    Err(_) => Err(Closed),
                },
                () = sleep_until(next) => self.send_due(&mut socket).await,
                message = socket.recv() => match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        send(&mut socket, &refusal(&bytes)).await
                    }
                    Some(Ok(Message::Text(_))) => send(&mut socket, &refusal(&[])).await,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => Ok(()),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => Err(Closed),
                },
            };
            if step.is_err() {
                let _ = socket.send(Message::Close(None)).await;
                return;
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

/// The error frame that answers a client frame. The channel takes no
/// client frame yet: presence records and subscriptions arrive with
/// ticket 55, and every other type, or bytes that aren't a frame, the
/// server never takes.
fn refusal(bytes: &[u8]) -> Frame {
    let zero = || IdOrZero([0; 16]);
    let error =
        |rejected_type: u8, domain: IdOrZero, session: IdOrZero, sequence: u64| Frame::Error {
            rejected_type,
            domain,
            session,
            sequence,
            code: Text("bad_request".into()),
            detail: None,
        };
    match Frame::from_bytes(bytes) {
        Ok(Frame::Presence { record }) => error(
            2,
            IdOrZero(record.body.domain.0),
            IdOrZero(record.body.session.0),
            record.body.sequence,
        ),
        Ok(frame) => error(frame.code(), zero(), zero(), 0),
        Err(_) => error(bytes.first().copied().unwrap_or(0), zero(), zero(), 0),
    }
}
