//! Presence on the desktop client (docs/workspace-format/sync-v2.md §23):
//! each collaborator's cursors and selections, encrypted under the presence
//! subkey of the domain's current epoch on a channel derived from the note's
//! `rid`, and gone 30 seconds after their last record.
//!
//! Selections travel as yrs relative positions, so a cursor stays on its
//! character as text changes around it; only the note's document in the
//! sandbox reads them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use sync_protocol::codec::{Bytes, List};
use sync_protocol::crypto::random_nonce;
use sync_protocol::messages::{Frame, MAX_SUBSCRIBE};
use sync_protocol::records::{
    MAX_POSITION_LEN, MAX_SELECTIONS, Presence, PresencePayload, PresenceState, Selection, Signed,
    presence_channel, within_clock_window,
};
use sync_protocol::types::{Id, Nonce};

use super::{SyncError, SyncService, WorkspaceSync, random, unix_now};

/// How long a cursor stays after its last record.
pub const CURSOR_LIFETIME: Duration = Duration::from_secs(30);

/// A selection of this device, in UTF-16 offsets of the note's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    /// Where the selection starts from.
    pub anchor: u32,
    /// Where the cursor sits.
    pub head: u32,
}

/// A collaborator's cursor in a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The collaborator's device.
    pub device: Id,
    /// A number that stays the same for that device, for its color.
    pub color: u8,
    /// Whether they're typing.
    pub active: bool,
    /// Their selections, in UTF-16 offsets of the note's text now.
    pub selections: Vec<SelectionRange>,
}

/// What a device last heard from one of a collaborator's sessions.
#[derive(Clone, Debug)]
struct Heard {
    rid: Id,
    state: PresenceState,
    selections: Vec<(Vec<u8>, Vec<u8>)>,
    at: Instant,
}

/// What a folder's sync client keeps of presence, in memory.
#[derive(Debug, Default)]
pub struct PresenceRuntime {
    /// This connection's session, drawn when the channel opens.
    session: Option<Id>,
    sequence: u64,
    /// Each open note's selections as they stand.
    selections: BTreeMap<Id, (Vec<SelectionRange>, bool)>,
    /// The selections each note last sent.
    sent: BTreeMap<Id, (Vec<SelectionRange>, bool)>,
    /// The channels the server knows of.
    subscribed: BTreeSet<Id>,
    /// What each collaborator's session last said.
    heard: BTreeMap<(Id, Id), Heard>,
    /// The highest sequence seen for each device and session during an
    /// epoch.
    sequences: BTreeMap<(Id, Id, Id, u64), u64>,
}

impl WorkspaceSync {
    /// A new connection: a new session, with no subscription yet.
    pub fn presence_connected(&mut self) -> Result<(), SyncError> {
        self.presence.session = Some(random(Id::random())?);
        self.presence.sequence = 0;
        self.presence.subscribed.clear();
        self.presence.sent.clear();
        Ok(())
    }

    /// Sets this device's selections in the note at `path`.
    pub fn set_selections(&mut self, path: &str, ranges: Vec<SelectionRange>, active: bool) {
        if let Some(rid) = self.rid_at(path) {
            self.set_note_selections(rid, ranges, active);
        }
    }

    /// Sets this device's selections in the note.
    pub fn set_note_selections(&mut self, rid: Id, ranges: Vec<SelectionRange>, active: bool) {
        self.presence.selections.insert(rid, (ranges, active));
    }

    /// Stops telling others of this device's selections in the note.
    pub(super) fn close_presence(&mut self, rid: &Id) {
        self.presence.selections.remove(rid);
        self.presence.sent.remove(rid);
    }

    /// The current epoch's key and record digest for `rid`'s domain.
    fn presence_epoch(
        &mut self,
        rid: &Id,
    ) -> Option<(
        Id,
        u64,
        sync_protocol::types::SecretKey,
        sync_protocol::types::Digest,
    )> {
        let domain = self.object_domain(rid);
        self.in_domain(domain, |sync| {
            let epoch = sync.state.epochs.last()?;
            let key = sync.keys.get(&epoch.body.epoch)?.clone();
            Some((sync.state.domain, epoch.body.epoch, key, epoch.digest()))
        })
    }

    /// The channels of the notes this device has open.
    fn wanted_channels(&mut self) -> BTreeMap<Id, Id> {
        let rids: Vec<Id> = self.text_open_notes();
        let mut channels = BTreeMap::new();
        for rid in rids.into_iter().take(MAX_SUBSCRIBE) {
            if let Some((_, _, key, _)) = self.presence_epoch(&rid) {
                channels.insert(presence_channel(&key, &rid), rid);
            }
        }
        channels
    }

    /// The frames to send on this tick: a subscribe when the open notes
    /// changed, and a presence record for each note whose selection changed.
    pub fn presence_frames(&mut self) -> Result<Vec<Frame>, SyncError> {
        let Some(session) = self.presence.session else {
            return Ok(Vec::new());
        };
        let mut frames = Vec::new();
        let channels = self.wanted_channels();
        let wanted: BTreeSet<Id> = channels.keys().copied().collect();
        if wanted != self.presence.subscribed {
            self.presence.subscribed = wanted;
            frames.push(Frame::Subscribe {
                channels: List(self.presence.subscribed.iter().copied().collect()),
            });
        }
        let changed: Vec<(Id, (Vec<SelectionRange>, bool))> = self
            .presence
            .selections
            .iter()
            .filter(|(rid, current)| self.presence.sent.get(rid) != Some(current))
            .map(|(rid, current)| (*rid, current.clone()))
            .collect();
        // At most two records a second, on the 500 ms tick (§23).
        if let Some((rid, (ranges, active))) = changed.into_iter().next() {
            if let Some(frame) = self.presence_record(session, rid, &ranges, active)? {
                frames.push(frame);
            }
            self.presence.sent.insert(rid, (ranges, active));
        }
        Ok(frames)
    }

    fn presence_record(
        &mut self,
        session: Id,
        rid: Id,
        ranges: &[SelectionRange],
        active: bool,
    ) -> Result<Option<Frame>, SyncError> {
        let Some((domain, epoch, key, record)) = self.presence_epoch(&rid) else {
            return Ok(None);
        };
        let Some(note) = self.text_shared(&rid) else {
            return Ok(None);
        };
        let mut selections = Vec::new();
        for range in ranges.iter().take(MAX_SELECTIONS) {
            let position = |index| note.encode_position(index, false).ok();
            let (Some(anchor), Some(head)) = (position(range.anchor), position(range.head)) else {
                continue;
            };
            if anchor.len() > MAX_POSITION_LEN || head.len() > MAX_POSITION_LEN {
                continue;
            }
            selections.push(Selection {
                anchor: Bytes(anchor),
                head: Bytes(head),
            });
        }
        self.presence.sequence += 1;
        let mut body = Presence {
            workspace: self.state.workspace,
            domain,
            epoch,
            device: self.credentials.keys.device(),
            session,
            sequence: self.presence.sequence,
            minute: (unix_now() / 60) as u32,
            channel: presence_channel(&key, &rid),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        let payload = PresencePayload {
            rid,
            state: if active {
                PresenceState::Active
            } else {
                PresenceState::Idle
            },
            selections: List(selections),
        };
        body.seal_payload(&payload, &key, &record, random(random_nonce())?)
            .map_err(|error| SyncError::Invalid(error.to_string()))?;
        Ok(Some(Frame::Presence {
            record: Signed::sign(body, self.credentials.keys.signing()),
        }))
    }

    /// Takes in a presence record, or drops it: the receivers' checks of
    /// §23 keep out a record under an old epoch, from outside the clock
    /// window, or that repeats an earlier sequence, as a server that
    /// forwards an old record would send. Returns whether it took it.
    pub fn take_presence(&mut self, record: &Signed<Presence>, now: Instant) -> bool {
        let body = &record.body;
        let channels = self.wanted_channels();
        let Some(rid) = channels.get(&body.channel).copied() else {
            return false;
        };
        let Some((domain, epoch, key, digest)) = self.presence_epoch(&rid) else {
            return false;
        };
        if body.domain != domain
            || body.workspace != self.state.workspace
            || body.epoch != epoch
            || !within_clock_window(body.minute, (unix_now() / 60) as u32)
        {
            return false;
        }
        let signed = self
            .remote
            .as_ref()
            .and_then(|remote| remote.policy.head_snapshot().devices.get(&body.device))
            .is_some_and(|device| record.verify(&device.card.signing_public).is_ok());
        if !signed {
            return false;
        }
        let position = (body.device, body.session, body.domain, body.epoch);
        if self
            .presence
            .sequences
            .get(&position)
            .is_some_and(|seen| body.sequence <= *seen)
        {
            return false;
        }
        let Ok(payload) = body.open_payload(&key, &digest) else {
            return false;
        };
        if payload.rid != rid {
            return false;
        }
        self.presence.sequences.insert(position, body.sequence);
        self.presence.heard.insert(
            (body.device, body.session),
            Heard {
                rid,
                state: payload.state,
                selections: payload
                    .selections
                    .iter()
                    .map(|s| (s.anchor.0.clone(), s.head.0.clone()))
                    .collect(),
                at: now,
            },
        );
        true
    }

    /// The session ended, so its cursors go.
    pub fn presence_gone(&mut self, device: Id, session: Id) {
        self.presence.heard.remove(&(device, session));
    }

    /// The collaborators' cursors in the note at `path` as of `now`: each
    /// goes 30 seconds after its last record.
    pub fn cursors(&mut self, path: &str, now: Instant) -> Vec<Cursor> {
        match self.rid_at(path) {
            Some(rid) => self.note_cursors(rid, now),
            None => Vec::new(),
        }
    }

    /// The collaborators' cursors in the note as of `now`.
    pub fn note_cursors(&mut self, rid: Id, now: Instant) -> Vec<Cursor> {
        self.presence
            .heard
            .retain(|_, heard| now.saturating_duration_since(heard.at) < CURSOR_LIFETIME);
        let Some(note) = self.text_shared(&rid) else {
            return Vec::new();
        };
        let mut cursors = Vec::new();
        for ((device, _), heard) in &self.presence.heard {
            if heard.rid != rid {
                continue;
            }
            let selections = heard
                .selections
                .iter()
                .filter_map(|(anchor, head)| {
                    Some(SelectionRange {
                        anchor: note.decode_position(anchor.clone()).ok()?,
                        head: note.decode_position(head.clone()).ok()?,
                    })
                })
                .collect();
            cursors.push(Cursor {
                device: *device,
                color: color_of(device),
                active: heard.state == PresenceState::Active,
                selections,
            });
        }
        cursors
    }

    /// Acts on the server's refusal of a presence record (§23): rotates a
    /// domain whose rotation is pending when the device holds the key, and
    /// fetches policy and keys otherwise.
    pub(super) async fn presence_refused(
        &mut self,
        service: &mut SyncService,
        code: &str,
    ) -> Result<(), SyncError> {
        match code {
            "rotation_pending" | "epoch_stale" | "forbidden" => {
                match self.refresh(service).await {
                    Ok(()) | Err(SyncError::KeyMissing) => {}
                    Err(error) => return Err(error),
                }
                if code == "rotation_pending"
                    && let Some(remote) = self.remote.as_mut()
                    && service.rotation_pending(remote)
                {
                    service.rotate_if_pending(remote).await?;
                    let remote = self.remote.take().expect("just used");
                    self.adopt(remote)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// A number from a device's ID, the same on every device, that picks its
/// color.
fn color_of(device: &Id) -> u8 {
    device.0[0]
}
